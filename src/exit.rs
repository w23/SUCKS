use {
    std::{
        //io::{Write, Read, Cursor, Seek, IoSlice, IoSliceMut},
        net::{ToSocketAddrs},
        // rc::Rc,
    },
    mio::{
        net::UdpSocket,
    },
    ::log::{info, trace, warn, error, debug},
    crate::{
        ste, log
    },
};

struct ExitUdpContext {
    socket: UdpSocket,
}

impl ExitUdpContext {
    fn listen(bind_addr: &str) -> std::io::Result<ExitUdpContext> {
        // TODO DNS
        let listen_addr = bind_addr.to_socket_addrs()?.next().unwrap();
        Ok(ExitUdpContext{
            socket: UdpSocket::bind(listen_addr)?
        })
    }
}

impl ste::Context for ExitUdpContext {
    fn registered(&mut self, ste: &mut ste::Ste, handle: ste::Handle) {
        ste.register_source(handle, &mut self.socket, 0).unwrap();
    }

    fn event(&mut self, ste: &mut ste::Ste, token: usize, event: &mio::event::Event) {
        log_scope!("udp");

        if event.is_readable() {
            const MAX_DATAGRAM_SIZE: usize = 65_507;
            let mut buf = vec![0u8; MAX_DATAGRAM_SIZE];
            let (len, addr) = match self.socket.recv_from(&mut buf) {
                Ok((len, addr)) => { (len, addr) },
                Err(e) => {
                    error!("{}", e);
                    return;
                }
            };
            buf.truncate(len);

            debug!("Got {} bytes from {}: {:?}", len, addr, buf);
        }
    }
}

pub fn main(listen: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut ste = ste::Ste::new(4).unwrap();
    let _context_handle = ste.register_context(Box::new(ExitUdpContext::listen(listen)?))?;
    ste.run()
}
