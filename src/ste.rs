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
    slab::Slab,
    log::{info, trace, warn, error, debug},
};

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

enum SocketData {
    Listener(Listen),
}

struct Socket {
    seq: usize,
    data: SocketData,
}

impl Socket {
    fn new(seq: usize, listen: Listen) -> Socket {
        Socket {
            seq,
            data: SocketData::Listener(listen),
        }
    }
}

pub struct Ste {
    poll: Poll,
    sockets: Slab::<Socket>,
    seq: usize,
}

const MAX_SOCKETS: usize = 128;

impl Ste {
    pub fn new() -> Result<Ste, Box<dyn std::error::Error>> {
        Ok(Ste {
            poll: Poll::new()?,
            sockets: Slab::<Socket>::with_capacity(MAX_SOCKETS),
            seq: 0,
        })
    }

    pub fn listen(&mut self, listener: Listen) -> Result<(), std::io::Error> {
        self.seq += 1;
        let index = self.sockets.insert(Socket::new(self.seq, listener));
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
        if let Some(ref mut sock) = self.sockets.get(index) {
            match &sock.data {
                SocketData::Listener(listen) => {
                    loop {
                        match listen.listener.accept() {
                            Ok((socket, _)) => {
                                error!("New socket {:?}, not implemented", socket);
                                (listen.callback)()?
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
                    }
                }
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
