use netlink_packet_sock_diag::{
    constants::*,
    unix::{UnixRequest, ShowFlags, StateFlags, nlas::Nla},
    NetlinkHeader,
    NetlinkMessage,
    NetlinkPayload,
    SockDiagMessage,
};
use netlink_sys::{protocols::NETLINK_SOCK_DIAG, Socket, SocketAddr};
use std::io;

pub fn get_socket_peer(socket_ino: u32) -> io::Result<u32> {
    let socket = Socket::new(NETLINK_SOCK_DIAG)?;
    socket.connect(&SocketAddr::new(0, 0))?;

    let mut packet = NetlinkMessage {
        header: NetlinkHeader {
            flags: NLM_F_REQUEST,
            ..Default::default()
        },
        payload: SockDiagMessage::UnixRequest(UnixRequest {
            state_flags: StateFlags::all(),
            inode: socket_ino,
            show_flags: ShowFlags::PEER,
            cookie: [0xff; 8]
        })
        .into()
    };

    packet.finalize();

    let mut buf = vec![0; packet.header.length as usize];

    // Before calling serialize, it is important to check that the buffer in which we're emitting is big
    // enough for the packet, other `serialize()` panics.
    assert_eq!(buf.len(), packet.buffer_len());

    packet.serialize(&mut buf[..]);
    socket.send(&buf[..], 0)?;

    let mut receive_buffer = vec![0; 4096];
    let mut offset = 0;
    while let Ok(size) = socket.recv(&mut receive_buffer[..], 0) {
        loop {
            let bytes = &receive_buffer[offset..];
            let rx_packet = <NetlinkMessage<SockDiagMessage>>::deserialize(bytes).unwrap();

            match rx_packet.payload {
                NetlinkPayload::Noop | NetlinkPayload::Ack(_) => {}
                NetlinkPayload::InnerMessage(SockDiagMessage::UnixResponse(response)) => {
                    let mut port: u32 = 0;
                    for nla in response.nlas {
                        match nla {
                            Nla::Peer(x) => { port = x; },
                            _ => ()
                        }
                    }
                    return Ok(port);
                },
                NetlinkPayload::InnerMessage(_) |  NetlinkPayload::Done => {
                    return Err(io::Error::new(io::ErrorKind::Other, "Unexpected response from netlink"));
                },
                NetlinkPayload::Error(err) =>
                {
                    return Err(io::Error::new(io::ErrorKind::Other, format!("Netlink error: {}", err.code)));
                },
                NetlinkPayload::Overrun(_) => {
                    return Err(io::Error::new(io::ErrorKind::Other, "Netlink overrun"));
                }
            }

            offset += rx_packet.header.length as usize;
            if offset == size || rx_packet.header.length == 0 {
                offset = 0;
                break;
            }
        }
    }

    return Err(io::Error::new(io::ErrorKind::Other, "Didn't get a response from netlink"));
}
