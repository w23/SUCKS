use {
    std::{
        //borrow::{BorrowMut},
        cell::RefCell,
        rc::Rc,
        io::{Write, Read},
        net::{/*IpAddr,*/ ToSocketAddrs},
        // ops::DerefMut,
    },
    mio::{
        Events, Poll, Token,
        // net::{
        //     UdpSocket,
        //     TcpStream, TcpListener,
        // },
    },
    log::{info, trace, warn, error, debug},
    ochenslab::OchenSlab,
};

struct Versioned<T> {
    seq: usize,
    value: T,
}

impl<T> Versioned<T> {
    fn new(seq: usize, value: T) -> Versioned<T> {
        Versioned { seq, value }
    }

    fn get(&self, seq: usize) -> Option<&T> {
        if self.seq != seq {
            return None;
        }

        return Some(&self.value);
    }

    fn get_mut(&mut self, seq: usize) -> Option<&mut T> {
        if self.seq != seq {
            return None;
        }

        return Some(&mut self.value);
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Handle {
    index: usize,
    seq: usize,
}

impl From<Token> for Handle {
    fn from(token: Token) -> Handle {
        Handle{
            index: token.0 & 0xffff,
            seq: token.0 >> 16
        }
    }
}

impl From<Handle> for Token {
    // FIXME limit seq bits
    fn from(handle: Handle) -> Token {
        assert!(handle.index < 0x10000);
        Token((handle.seq << 16) | handle.index)
    }
}

pub struct Sequence {
    seq: usize,
}

impl Sequence {
    fn new() -> Sequence {
        Sequence { seq: 0 }
    }

    fn next(&mut self) -> usize {
        let ret = self.seq;
        self.seq += 1;
        ret
    }
}

struct SourceMapping {
    context: Handle,
    token: usize,
}

struct VersionedSlab<T> {
    slab: OchenSlab::<Versioned<T>>,
}

impl<T> VersionedSlab<T> {
    fn with_capacity(capacity: usize) -> VersionedSlab<T> {
        VersionedSlab {
            slab: OchenSlab::with_capacity(capacity)
        }
    }

    fn get_ref_by_handle(&self, handle: Handle) -> Option<&T> {
        let value = match self.slab.get(handle.index) {
            Some(value) => value,
            None => {
                warn!("S{}: stale event, no such value", handle.index);
                return None;
            }
        };

        let value: &T = match value.get(handle.seq) {
            Some(value) => value,
            None => {
                warn!("S{} stale seq {} received, slot has {}", handle.index, handle.seq, value.seq);
                return None;
            }
        };

        Some(value)
    }

    fn get_ref_mut_by_handle(&mut self, handle: Handle) -> Option<&mut T> {
        let value = match self.slab.get_mut(handle.index) {
            Some(value) => value,
            None => {
                warn!("S{}: stale event, no such value", handle.index);
                return None;
            }
        };

        let value_seq = value.seq;
        let value: &mut T = match value.get_mut(handle.seq) {
            Some(value) => value,
            None => {
                warn!("S{} stale seq {} received, slot has {}", handle.index, handle.seq, value_seq);
                return None;
            }
        };

        Some(value)
    }

    // FIXME add insert, remove with seq
}

pub trait Context {
    fn registered(&mut self, ste: &mut Ste, handle: Handle);
    fn event(&mut self, ste: &mut Ste, token: usize, event: &mio::event::Event);
}

pub struct Ste {
    poll: Poll,
    mapping_seq: Sequence,
    contexts_seq: Sequence, // FIXME hide these into versioned slab
    mapping: VersionedSlab<SourceMapping>,
    contexts: VersionedSlab<Rc<RefCell<Box<dyn Context>>>>,
}

impl Ste {
    pub fn new(max_contexts: usize) -> Result<Ste, Box<dyn std::error::Error>> {
        Ok(Ste {
            poll: Poll::new()?,
            mapping_seq: Sequence::new(),
            contexts_seq: Sequence::new(),
            mapping: VersionedSlab::with_capacity(max_contexts),
            contexts: VersionedSlab::with_capacity(max_contexts),
        })
    }

    pub fn register_context(&mut self, context: Box<dyn Context>) -> Result<Handle, std::io::Error> {
        let seq = self.contexts_seq.next();
        let index = match self.contexts.slab.insert(Versioned::new(seq, Rc::new(RefCell::new(context)))) {
            None => { return Err(std::io::Error::new(std::io::ErrorKind::Other, "Capacity exceeded")); },
            Some(index) => index
        };

        info!("C{} seq={}", index, seq);
        let handle = Handle{index, seq};
        let context = self.contexts.get_ref_mut_by_handle(handle).unwrap().clone();
        context.borrow_mut().registered(self, handle);
        Ok(handle)
    }

    pub fn register_source(&mut self, context: Handle, source: &mut dyn mio::event::Source, token: usize) -> Result<Handle, std::io::Error> {
        const INTERESTS: mio::Interest = mio::Interest::READABLE.add(mio::Interest::WRITABLE);

        if self.contexts.get_ref_by_handle(context).is_none() {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "Context not found"));
        }

        let seq = self.mapping_seq.next();
        let mapping = match self.mapping.slab.insert(Versioned::new(seq, SourceMapping{context: context, token: token})) {
            Some(mapping) => mapping,
            None => {
                return Err(std::io::Error::new(std::io::ErrorKind::Other, "Sources capacity exceeded"));
            }
        };

        let handle = Handle{index: mapping, seq: seq};
        match self.poll.registry().register(source, Token::from(handle), INTERESTS) {
            Ok(_) => Ok(handle),
            Err(e) => {
                self.mapping.slab.remove(mapping);
                Err(e)
            },
        }
    }

    // TODO RegisteredSource type?
    pub fn deregister_source(&mut self, source_handle: Handle, source: &mut dyn mio::event::Source) -> std::io::Result<()> {
        let mapping = match self.mapping.get_ref_by_handle(source_handle) {
            None => return Err(std::io::Error::new(std::io::ErrorKind::Other, "Source not found")),
            Some(mapping) => mapping,
        };

        self.mapping.slab.remove(source_handle.index);
        self.poll.registry().deregister(source)
    }

    pub fn deregister_context(&mut self, context: Handle) -> std::io::Result<()> {
        // FIXME check that all sources have been deregistered
        if self.contexts.get_ref_by_handle(context).is_none() {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "Context not found"));
        }

        // FIXME native remove
        self.contexts.slab.remove(context.index);
        Ok(())
    }

    // FIXME deregister:
    // -- all sources by context?
    // -- source by handle?
    // O(N) :((( or index

    // fn handleListener(&mut self, socket_handle: SocketHandle, event: &mio::event::Event) {
    //     let socket = match get_ref_by_handle(&self.listeners, socket_handle) {
    //         Some(sock) => sock,
    //         None => return,
    //     };
    //
    // }
    //
    // TODO: need to separate missing socket and missing context
    // fn get_socket_context_mut(&mut self, socket_handle: SocketHandle) -> Option<&mut Box<dyn Context>> {
    //     let socket = match get_ref_by_handle(&self.listeners, socket_handle) {
    //         Some(sock) => sock,
    //         None => return None,
    //     };
    //
    //     get_ref_mut_by_handle(&mut self.contexts, socket.context)
    // }

    // fn handleStream(&mut self, socket_handle: SocketHandle, event: &mio::event::Event) {
    //     let socket = match get_ref_mut_by_handle(&mut self.streams, socket_handle) {
    //         Some(sock) => sock,
    //         None => return,
    //     };
    //
    //     let context = match get_ref_mut_by_handle(&mut self.contexts, socket.context) {
    //         Some(context) => context,
    //         None => {
    //             error!("C{} is stale, but associated socket still exists", socket.context.index);
    //             unimplemented!("FIXME this socket should die now");
    //             //return;
    //         }
    //     };
    //
    //     if event.is_error() {
    //         unimplemented!();
    //     }
    //
    //     if event.is_readable() {
    //         let buf = context.get_buffer();
    //         let read = match socket.socket.read(buf) {
    //             Ok(read) => read,
    //             Err(e) => {
    //                 error!("Error: {:?}", e);
    //                 0
    //             },
    //         };
    //         context.buffer_read(self, read);
    //     }
    // }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut events = Events::with_capacity(128);

        loop {
            trace!("loop. sources={}", self.mapping.slab.len());
            match self.poll.poll(&mut events, None) {
                Err(e) => {
                    error!("poll error: {:?}", e);
                    continue;
                },
                _ => {},
            }

            for event in &events {
                debug!("event: {:?}", event);
                let mapping = match self.mapping.get_ref_by_handle(Handle::from(event.token())) {
                    Some(mapping) => mapping,
                    None => {
                        warn!("Stale mapping token {:?}", event.token());
                        // TODO: unregister?
                        continue;
                    }
                };

                debug!("context={:?}", mapping.context);

                let context = match self.contexts.get_ref_mut_by_handle(mapping.context) {
                    Some(context) => context.clone(),
                    None => {
                        warn!("Stale context handle {:?}", mapping.context);
                        // TODO: unregister?
                        continue;
                    }
                };

                context.borrow_mut().event(self, mapping.token, event);
            }
        }
    }
}
