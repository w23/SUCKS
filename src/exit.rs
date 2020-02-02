use tokio::net::{UdpSocket};
use std::io::Cursor;
use byteorder::ReadBytesExt;

async fn connectionCreate(addr: &std::net::SocketAddr, cur: &mut Cursor<&[u8]>) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

async fn connectionHandle(addr: &std::net::SocketAddr, cur: &mut Cursor<&[u8]>) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

pub async fn main(listen: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut socket = UdpSocket::bind(listen).await?;

    loop {
        const MAX_DATAGRAM_SIZE: usize = 65_507;
        let mut buf = vec![0u8; MAX_DATAGRAM_SIZE];
        let (len, addr) = socket.recv_from(&mut buf).await?;

        let mut cursor = Cursor::new(&buf[0..len]);
        let session_id = ReadBytesExt::read_u8(&mut cursor)?;

        if session_id == 0 {
            connectionCreate(&addr, &mut cursor).await?;
        } else {
            connectionHandle(&addr, &mut cursor).await?;
        }
    }
}
