use {
    std::{
        io::{Write, Read, Cursor, Seek},
        net::{IpAddr, ToSocketAddrs},
    },
    mio::{
        Events, Poll, PollOpt, Ready, Token,
        net::{
            //UdpSocket,
            TcpStream, TcpListener,
        },
    },
    log::{info, trace, warn, error, debug},
};

trait StreamHandler {
    fn push(&mut self);
    fn pull(&mut self);
}

//type ListenCallback = dyn Fn() -> Result<Box<dyn StreamHandler>, std::io::Error>;
type ListenCallback = dyn Fn() -> Result<(), std::io::Error>;

pub struct Listen
{
    listener: mio::net::TcpListener,
    callback: Box<ListenCallback>,
}

impl Listen {
    pub fn new(bind_addr: &str, callback: Box<ListenCallback>) -> Result<Listen, Box<dyn std::error::Error>> {
        let listen_addr = bind_addr.to_socket_addrs()?.next().unwrap();
        Ok(Listen {
            listener: mio::net::TcpListener::bind(&listen_addr)?,
            callback: callback,
        })
    }
}

struct Stream {
    stream_socket: TcpStream,
    stream_handler: Box<dyn StreamHandler>
}

impl Stream {
    fn new(stream_socket: TcpStream, stream_handler: Box<dyn StreamHandler>) -> Stream {
        Stream { stream_socket, stream_handler }
    }
}

enum SocketData {
    Listener(Listen),
    Stream(Stream),
}

struct Socket {
    seq: usize,
    data: SocketData,
}

impl Socket {
    fn from_listen(seq: usize, listen: Listen) -> Socket {
        Socket {
            seq,
            data: SocketData::Listener(listen),
        }
    }
    fn from_stream(seq: usize, stream: Stream) -> Socket {
        Socket {
            seq,
            data: SocketData::Stream(stream),
        }
    }
}

struct OchenSlab<T> {
    storage: Vec<Option<T>>,
    free: Vec<usize>,
}

impl<T> OchenSlab<T> {
    fn with_capacity(capacity: usize) -> OchenSlab<T> {
        let mut storage = Vec::<Option<T>>::with_capacity(capacity);
        storage.resize_with(capacity, || None);
        let mut free = Vec::<usize>::with_capacity(capacity);
        let mut i = 0 as usize;
        free.resize_with(capacity, || {
            let value = capacity - 1 - i;
            i += 1;
            value
        });

        OchenSlab {
            storage, free
        }
    }

    fn len(&self) -> usize {
        self.storage.len() - self.free.len()
    }

    fn get(&self, index: usize) -> Option<&T> {
        if index >= self.storage.len() { return None; }

        self.storage[index].as_ref()
    }

    fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.storage.len() { return None; }

        self.storage[index].as_mut()
    }

    fn insert(&mut self, t: T) -> Option<usize> {
        let index = self.free.pop();
        if index.is_none() { return None; }
        let index = index.unwrap();

        self.storage[index] = Some(t);
        Some(index)
    }

    fn remove(&mut self, index: usize) -> Option<T> {
        if index >= self.storage.len() { return None; }

        let value = self.storage[index].take();
        if value.is_some() {
            self.free.push(index);
        }

        value
    }
}

pub struct Ste {
    poll: Poll,
    sockets: OchenSlab::<Socket>,
    seq: usize,
}

const MAX_SOCKETS: usize = 128;

impl Ste {
    pub fn new() -> Result<Ste, Box<dyn std::error::Error>> {
        Ok(Ste {
            poll: Poll::new()?,
            sockets: OchenSlab::<Socket>::with_capacity(MAX_SOCKETS),
            seq: 0,
        })
    }

    fn get_seq(&mut self) -> usize {
        self.seq += 1;
        self.seq
    }

    pub fn listen(&mut self, listener: Listen) -> Result<(), std::io::Error> {
        let seq = self.get_seq();
        let index = self.sockets.insert(Socket::from_listen(seq, listener));
        if index.is_none() { return Err(std::io::Error::new(std::io::ErrorKind::Other, "Capacity exceeded")); }
        let index = index.unwrap();
        let sock = self.sockets.get_mut(index).unwrap();
        let token = Token(index + (self.seq << 16));
        if let SocketData::Listener(listen) = &sock.data {
            info!("C{}: token {} for socket {:?}", sock.seq, token.0, listen.listener);
            self.poll.register(&listen.listener, token,
                Ready::readable() | Ready::writable(), PollOpt::edge())
        } else {
            panic!("Stored Listener value, restored non-Listener");
        }
    }

    fn handleSocketIndex(&mut self, index: usize, seq: usize) -> Result<(), Box<dyn std::error::Error>> {
        // TODO how to nest less
        if let Some(ref mut sock) = self.sockets.get(index) {
            match &sock.data {
                SocketData::Listener(listen) => loop {
                    match listen.listener.accept() {
                        Ok((socket, _)) => {
                            let stream_handler = (listen.callback)()?;
                            error!("New socket {:?} not implemented", socket);
                            //let seq = self.get_seq();
                            //let mut stream = Stream::new(socket, stream_handler);
                            //let socket_obj = Socket::from_stream(seq, stream);
                            /*
                            conn_seq += 1;
                            let index = connections.insert(Connection::new(conn_seq, socket));
                            let token = Token(index + (conn_seq << 16) as usize);
                            let conn = connections.get_mut(index).unwrap();
                            info!("C{}: token {} for socket {:?}", conn.seq, token.0, conn.client_stream);
                            poll.register(&conn.client_stream, token,
                                Ready::readable() | Ready::writable(), PollOpt::edge())?;
                            */
                        },
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                        Err(e) => return Err(Box::new(e))
                    }
                },
                SocketData::Stream(stream) => loop {
                },
            }
        } else {
            warn!("Stale event for C{}", seq);
        }

        Ok(())
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut events = Events::with_capacity(128);

        loop {
            trace!("loop. sockets={}", self.sockets.len());
            self.poll.poll(&mut events, None).unwrap();

            for event in &events {
                debug!("event: {:?}", event);
                let t = event.token().0;
                let seq = t >> 16;
                let index = (t & 0xffff) % MAX_SOCKETS;

                self.handleSocketIndex(index, seq)?
            }
        }
    }
}
