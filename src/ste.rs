use {
    std::{
        //borrow::{BorrowMut},
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

const INTERESTS: mio::Interest = mio::Interest::READABLE.add(mio::Interest::WRITABLE);

#[derive(Debug)]
enum SocketType {
    TcpListener = 1,
    TcpStream = 2,
}

// FIXME limit seq bits
fn make_token(index: usize, seq: usize, kind: SocketType) -> Token {
    assert!(index < 0x10000);
    Token((seq << 18) | (index << 2) | kind as usize)
}

fn parse_token(token: Token) -> (usize, usize, SocketType) {
    let kind = match token.0 & 0x3 {
        1 => SocketType::TcpListener,
        2 => SocketType::TcpStream,
        _ => panic!("Unexpected value"),
    };
    ((token.0 >> 2) & 0xffff, token.0 >> 18, kind)
}

struct SocketTcpListen {
    context: ContextHandle,
    socket: mio::net::TcpListener,
}

impl SocketTcpListen {
    fn new(context: ContextHandle, bind_addr: &str) -> Result<SocketTcpListen, std::io::Error> {
        let listen_addr = bind_addr.to_socket_addrs()?.next().unwrap();
        Ok(SocketTcpListen{ context, socket: mio::net::TcpListener::bind(listen_addr)?})
    }

    fn register(&mut self, registry: &mio::Registry, index: usize, seq: usize) -> std::io::Result<()> {
        let token = make_token(index, seq, SocketType::TcpListener);
        info!("S{}: token {} for socket", seq, token.0);
        registry.register(&mut self.socket, token, INTERESTS)
    }
}

struct SocketTcpStream {
    context: ContextHandle,
    socket: mio::net::TcpStream,
}

impl SocketTcpStream {
    fn from_stream(context: ContextHandle, socket: mio::net::TcpStream) -> SocketTcpStream {
        SocketTcpStream { context, socket }
    }

    fn register(&mut self, registry: &mio::Registry, index: usize, seq: usize) -> std::io::Result<()> {
        let token = make_token(index, seq, SocketType::TcpStream);
        info!("S{}: token {} for socket", seq, token.0);
        registry.register(&mut self.socket, token, INTERESTS)
    }
}

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

pub trait Context {
    fn registered(&mut self, handle: ContextHandle);
    fn accept(&mut self, socket: SocketHandle) -> Option<Box<dyn Context>>;
    fn get_buffer(&mut self) -> &mut [u8];
    fn buffer_read(&mut self, read: usize);
    //fn event(&mut self, socket: SocketHandle);
    // pub registered: Box<dyn FnMut(StreamSocket)>,
    // pub push: Box<dyn FnMut(&[u8]) -> usize>,
    // pub pull: Box<dyn FnMut(&mut [u8]) -> usize>,
    // pub error: Box<dyn FnMut(std::io::Error)>,
}

trait Handle {
    fn index(&self) -> usize;
    fn seq(&self) -> usize;
}

#[derive(Debug, Copy, Clone)]
pub struct ContextHandle {
    index: usize,
    seq: usize,
}

impl ContextHandle {
    fn new(index: usize, seq: usize) -> ContextHandle {
        ContextHandle { index, seq }
    }
}

impl Handle for ContextHandle {
    fn index(&self) -> usize { return self.index; }
    fn seq(&self) -> usize { return self.seq; }
}

#[derive(Debug, Copy, Clone)]
pub struct SocketHandle {
    index: usize,
    seq: usize,
}

impl SocketHandle {
    fn new(index: usize, seq: usize) -> SocketHandle {
        SocketHandle { index, seq }
    }
}

impl Handle for SocketHandle {
    fn index(&self) -> usize { return self.index; }
    fn seq(&self) -> usize { return self.seq; }
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

type TcpListenSocketsContainer = OchenSlab::<Versioned<SocketTcpListen>>;
type TcpStreamSocketsContainer = OchenSlab::<Versioned<SocketTcpStream>>;
type ContextsContainer = OchenSlab::<Versioned<Box<dyn Context>>>;

pub struct Ste {
    poll: Poll,
    seq: Sequence,
    listeners: TcpListenSocketsContainer,
    streams: TcpStreamSocketsContainer,
    contexts: ContextsContainer,
}

fn get_ref_by_handle<H: Handle, S>(slab: &OchenSlab::<Versioned<S>>, handle: H) -> Option<&S> {
    let value = match slab.get(handle.index()) {
        Some(value) => value,
        None => {
            warn!("S{}: stale event, no such value", handle.index());
            return None;
        }
    };

    let value: &S = match value.get(handle.seq()) {
        Some(value) => value,
        None => {
            warn!("S{} stale seq {} received, slot has {}", handle.index(), handle.seq(), value.seq);
            return None;
        }
    };

    return Some(value);
}

fn get_ref_mut_by_handle<H: Handle, S>(slab: &mut OchenSlab::<Versioned<S>>, handle: H) -> Option<&mut S> {
    let value = match slab.get_mut(handle.index()) {
        Some(value) => value,
        None => {
            warn!("S{}: stale event, no such value", handle.index());
            return None;
        }
    };

    let value_seq = value.seq;
    let value: &mut S = match value.get_mut(handle.seq()) {
        Some(value) => value,
        None => {
            warn!("S{} stale seq {} received, slot has {}", handle.index(), handle.seq(), value_seq);
            return None;
        }
    };

    return Some(value);
}

impl Ste {
    pub fn new(max_sockets: usize) -> Result<Ste, Box<dyn std::error::Error>> {
        Ok(Ste {
            poll: Poll::new()?,
            seq: Sequence::new(),
            listeners: TcpListenSocketsContainer::with_capacity(4),
            streams: TcpStreamSocketsContainer::with_capacity(max_sockets),
            contexts: ContextsContainer::with_capacity(max_sockets), // FIXME max_contexts
        })
    }

    pub fn register_context(&mut self, context: Box<dyn Context>) -> Result<ContextHandle, std::io::Error> {
        let seq = self.seq.next();
        let index = match self.contexts.insert(Versioned::<Box<dyn Context>>::new(seq, context)) {
            None => { return Err(std::io::Error::new(std::io::ErrorKind::Other, "Capacity exceeded")); },
            Some(index) => index
        };

        info!("C{}: [{}]", seq, index);
        let handle = ContextHandle::new(index, seq);
        self.contexts.get_mut(index).unwrap().get_mut(seq).unwrap().registered(handle);
        Ok(handle)
    }

    pub fn connect_stream(&mut self, address: &str, context: ContextHandle) -> Result<SocketHandle, std::io::Error> {
        unimplemented!();
    }

    pub fn listen(&mut self, address: &str, context: ContextHandle) -> Result<SocketHandle, std::io::Error> {
        let socket = SocketTcpListen::new(context, address)?;
        let seq = self.seq.next();
        let index = match self.listeners.insert(Versioned::new(seq, socket)) {
            None => { return Err(std::io::Error::new(std::io::ErrorKind::Other, "Capacity exceeded")); },
            Some(index) => index
        };

        let socket = self.listeners.get_mut(index).unwrap().get_mut(seq).unwrap();
        socket.register(&self.poll.registry(), index, seq)?;
        Ok(SocketHandle::new(index, seq))
    }

    fn handleListener(&mut self, socket_handle: SocketHandle, event: &mio::event::Event) {
        let socket = match get_ref_by_handle(&self.listeners, socket_handle) {
            Some(sock) => sock,
            None => return,
        };

        loop {
            let accepted = match socket.socket.accept() {
                Ok((socket, _)) => socket,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                Err(e) => {
                    error!("S{}, cannot accept: {:?}", socket_handle.index, e);
                    // FIXME notify context
                    return;
                }
            };

            let seq = self.seq.next();
            let index = match self.streams.insert(Versioned::new(seq, SocketTcpStream::from_stream(socket.context, accepted))) {
                Some(index) => index,
                None => {
                    error!("S{}, cannot insert new socket: max sockets reached", socket_handle.index);
                    continue;
                }
            };

            let new_socket_handle = SocketHandle{index, seq};

            let context = match get_ref_mut_by_handle(&mut self.contexts, socket.context) {
                Some(context) => context,
                None => {
                    error!("C{} is stale, but associated socket still exists", socket.context.index);
                    unimplemented!("FIXME this socket should die now");
                    //return;
                }
            };

            let context = context.accept(SocketHandle{index, seq});
            if context.is_some() {
                let seq = self.seq.next();
                let index = match self.contexts.insert(Versioned::new(seq, context.unwrap())) {
                    Some(index) => index,
                    None => {
                        error!("Cannot store new context");
                        unimplemented!("FIXME destroy new socket");
                        //continue;
                    },
                };

                self.contexts.get_mut(index).unwrap().get_mut(seq).unwrap().registered(ContextHandle{index, seq});

                {
                    info!("registering socket index={} seq={}", new_socket_handle.index, new_socket_handle.seq);
                    let socket = self.streams.get_mut(new_socket_handle.index).unwrap().get_mut(new_socket_handle.seq).unwrap();
                    socket.register(&self.poll.registry(), new_socket_handle.index, new_socket_handle.seq).unwrap();
                    socket.context = ContextHandle{index, seq};
                }
            }
        }
    }

    // TODO: need to separate missing socket and missing context
    // fn get_socket_context_mut(&mut self, socket_handle: SocketHandle) -> Option<&mut Box<dyn Context>> {
    //     let socket = match get_ref_by_handle(&self.listeners, socket_handle) {
    //         Some(sock) => sock,
    //         None => return None,
    //     };
    //
    //     get_ref_mut_by_handle(&mut self.contexts, socket.context)
    // }

    fn handleStream(&mut self, socket_handle: SocketHandle, event: &mio::event::Event) {
        let socket = match get_ref_mut_by_handle(&mut self.streams, socket_handle) {
            Some(sock) => sock,
            None => return,
        };

        let context = match get_ref_mut_by_handle(&mut self.contexts, socket.context) {
            Some(context) => context,
            None => {
                error!("C{} is stale, but associated socket still exists", socket.context.index);
                unimplemented!("FIXME this socket should die now");
                //return;
            }
        };

        if event.is_error() {
            unimplemented!();
        }

        if event.is_readable() {
            let buf = context.get_buffer();
            let read = match socket.socket.read(buf) {
                Ok(read) => read,
                Err(e) => {
                    error!("Error: {:?}", e);
                    0
                },
            };
            context.buffer_read(read);
        }
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut events = Events::with_capacity(128);

        loop {
            trace!("loop. listeners={} streams={}", self.listeners.len(), self.streams.len());
            match self.poll.poll(&mut events, None) {
                Err(e) => {
                    error!("poll error: {:?}", e);
                    continue;
                },
                _ => {},
            }

            for event in &events {
                debug!("event: {:?}", event);
                let (index, seq, socket_type) = parse_token(event.token());
                debug!("index={} seq={} type={:?}", index, seq, socket_type);
                let socket_handle = SocketHandle{ index, seq };

                match socket_type {
                    SocketType::TcpListener => self.handleListener(socket_handle, event),
                    SocketType::TcpStream => self.handleStream(socket_handle, event),
                }
            }
        }
    }
}
