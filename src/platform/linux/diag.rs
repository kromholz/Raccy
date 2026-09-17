// `/proc/net/tcp` says who talks to whom but not how much; `SOCK_DIAG_BY_FAMILY`
// answers with the kernel's own per-socket counters, and needs no privileges.
// TCP only: the kernel keeps no such counters for UDP sockets.

use std::collections::HashMap;
use std::ffi::c_void;

// What a socket has carried since it was opened: taken in, sent out.
pub(super) type Carried = HashMap<u64, (u64, u64)>;

// Every TCP socket's byte counters, by the inode that `/proc/net/tcp` and
// `/proc/<pid>/fd` both name it with.
pub(super) fn socket_bytes() -> Option<Carried> {
    // Each family is asked for on its own and counts only if the kernel saw
    // it through: half of a refused dump is not a count of anything, and it
    // must not be mistaken for the sockets simply having carried nothing.
    let (v4, v6) = (dump(AF_INET), dump(AF_INET6));
    match (v4, v6) {
        (None, None) => None,
        (some, other) => {
            let mut out = some.unwrap_or_default();
            out.extend(other.unwrap_or_default());
            Some(out)
        }
    }
}

// How a dump ended: the kernel said that was all of them, it refused, or
// there is more to read.
#[derive(PartialEq)]
enum End {
    Done,
    Refused,
    More,
}

// The sockets of one family, or nothing at all when the kernel refused: a
// kernel without tcp_diag answers the request with an error, and that is not
// an answer with no sockets in it.
fn dump(family: u8) -> Option<Carried> {
    let socket = Netlink::open()?;
    if !socket.ask(family) {
        return None;
    }
    let mut out = HashMap::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match socket.read(&mut buffer) {
            Some(read) if read > 0 => match take_sockets(&buffer[..read], &mut out) {
                End::Done => return Some(out),
                End::Refused => return None,
                End::More => {}
            },
            // The read timed out or gave nothing: what came in so far stands.
            _ => return (!out.is_empty()).then_some(out),
        }
    }
}

// The sockets of one read into `out`, and how the kernel ended it.
fn take_sockets(mut messages: &[u8], out: &mut Carried) -> End {
    while messages.len() >= HEADER {
        let len = u32::from_ne_bytes(messages[0..4].try_into().unwrap_or_default()) as usize;
        let kind = u16::from_ne_bytes(messages[4..6].try_into().unwrap_or_default());
        // A message cut in half is not the kernel saying it is finished.
        if len < HEADER || len > messages.len() {
            return End::More;
        }
        match kind {
            NLMSG_DONE => return End::Done,
            NLMSG_ERROR => return End::Refused,
            SOCK_DIAG_BY_FAMILY => {
                if let Some((inode, carried)) = socket_of(&messages[HEADER..len]) {
                    out.insert(inode, carried);
                }
            }
            _ => {}
        }
        messages = &messages[aligned(len).min(messages.len())..];
    }
    End::More
}

// Nothing for a socket the kernel sent no counters for, like one just opened.
fn socket_of(message: &[u8]) -> Option<(u64, (u64, u64))> {
    let inode = u64::from(u32::from_ne_bytes(message.get(INODE_AT..INODE_AT + 4)?.try_into().ok()?));
    let mut at = DIAG_MSG;
    while at + 4 <= message.len() {
        let len = u16::from_ne_bytes(message[at..at + 2].try_into().ok()?) as usize;
        let kind = u16::from_ne_bytes(message[at + 2..at + 4].try_into().ok()?);
        if len < 4 || at + len > message.len() {
            break;
        }
        if kind == INET_DIAG_INFO {
            let info = &message[at + 4..at + len];
            if info.len() >= BYTES_RECEIVED + 8 {
                let sent = u64::from_ne_bytes(info[BYTES_ACKED..BYTES_ACKED + 8].try_into().ok()?);
                let taken = u64::from_ne_bytes(info[BYTES_RECEIVED..BYTES_RECEIVED + 8].try_into().ok()?);
                return Some((inode, (taken, sent)));
            }
        }
        at += aligned(len);
    }
    None
}

// Netlink counts its lengths up to the next four bytes.
fn aligned(len: usize) -> usize {
    (len + 3) & !3
}

// The netlink header, sixteen bytes: length, kind, flags, order, sender.
const HEADER: usize = 16;
// `struct inet_diag_msg`, before the attributes that follow it.
const DIAG_MSG: usize = 72;
// Where that struct keeps the socket's inode.
const INODE_AT: usize = 68;
// `struct tcp_info`: where the kernel keeps the two counters. Both have
// been at these places since Linux 4.1.
const BYTES_ACKED: usize = 120;
const BYTES_RECEIVED: usize = 128;

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const SOCK_DIAG_BY_FAMILY: u16 = 20;
const INET_DIAG_INFO: u16 = 2;
// Ask for that one attribute: `1 << (INET_DIAG_INFO - 1)`.
const EXT_INFO: u8 = 2;
const AF_NETLINK: i32 = 16;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;
const SOCK_DGRAM: i32 = 2;
const NETLINK_SOCK_DIAG: i32 = 4;
const IPPROTO_TCP: u8 = 6;
const NLM_F_REQUEST: u16 = 0x001;
const NLM_F_DUMP: u16 = 0x300;
// Sockets in any state at all.
const ALL_STATES: u32 = u32::MAX;
const SOL_SOCKET: i32 = 1;
const SO_RCVTIMEO: i32 = 20;

struct Netlink(i32);

impl Netlink {
    fn open() -> Option<Netlink> {
        let fd = unsafe { socket(AF_NETLINK, SOCK_DGRAM, NETLINK_SOCK_DIAG) };
        if fd < 0 {
            return None;
        }
        let timeout = TimeVal { seconds: 0, micros: 200_000 };
        unsafe { setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout as *const _ as *const c_void, size_of::<TimeVal>() as u32) };
        Some(Netlink(fd))
    }

    fn ask(&self, family: u8) -> bool {
        let mut request = [0u8; HEADER + 56];
        let len = request.len() as u32;
        request[0..4].copy_from_slice(&len.to_ne_bytes());
        request[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        request[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_DUMP).to_ne_bytes());
        request[HEADER] = family;
        request[HEADER + 1] = IPPROTO_TCP;
        request[HEADER + 2] = EXT_INFO;
        request[HEADER + 4..HEADER + 8].copy_from_slice(&ALL_STATES.to_ne_bytes());
        // To the kernel, which is address nothing.
        let kernel = SockAddrNl { family: AF_NETLINK as u16, pad: 0, pid: 0, groups: 0 };
        let sent = unsafe {
            sendto(
                self.0,
                request.as_ptr() as *const c_void,
                request.len(),
                0,
                &kernel as *const _ as *const c_void,
                size_of::<SockAddrNl>() as u32,
            )
        };
        sent == request.len() as isize
    }

    fn read(&self, buffer: &mut [u8]) -> Option<usize> {
        let read = unsafe { recv(self.0, buffer.as_mut_ptr() as *mut c_void, buffer.len(), 0) };
        (read > 0).then_some(read as usize)
    }
}

impl Drop for Netlink {
    fn drop(&mut self) {
        unsafe { close(self.0) };
    }
}

#[repr(C)]
struct SockAddrNl {
    family: u16,
    pad: u16,
    pid: u32,
    groups: u32,
}

#[repr(C)]
struct TimeVal {
    seconds: i64,
    micros: i64,
}

unsafe extern "C" {
    fn socket(domain: i32, kind: i32, protocol: i32) -> i32;
    fn setsockopt(fd: i32, level: i32, name: i32, value: *const c_void, len: u32) -> i32;
    fn sendto(fd: i32, buf: *const c_void, len: usize, flags: i32, to: *const c_void, to_len: u32) -> isize;
    fn recv(fd: i32, buf: *mut c_void, len: usize, flags: i32) -> isize;
    fn close(fd: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dump_gives_the_bytes_of_each_socket() {
        let mut info = vec![0u8; 160];
        info[BYTES_ACKED..BYTES_ACKED + 8].copy_from_slice(&4096u64.to_ne_bytes());
        info[BYTES_RECEIVED..BYTES_RECEIVED + 8].copy_from_slice(&1_048_576u64.to_ne_bytes());
        let mut message = vec![0u8; DIAG_MSG];
        message[INODE_AT..INODE_AT + 4].copy_from_slice(&12345u32.to_ne_bytes());
        message.extend_from_slice(&((info.len() + 4) as u16).to_ne_bytes());
        message.extend_from_slice(&INET_DIAG_INFO.to_ne_bytes());
        message.extend_from_slice(&info);

        let mut dump = Vec::new();
        dump.extend_from_slice(&((HEADER + message.len()) as u32).to_ne_bytes());
        dump.extend_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        dump.extend_from_slice(&[0u8; 10]);
        dump.extend_from_slice(&message);
        dump.extend_from_slice(&(HEADER as u32).to_ne_bytes());
        dump.extend_from_slice(&NLMSG_DONE.to_ne_bytes());
        dump.extend_from_slice(&[0u8; 10]);

        let mut out = Carried::new();
        assert!(take_sockets(&dump, &mut out) == End::Done, "the kernel said that was the last of them");
        assert_eq!(out.get(&12345), Some(&(1_048_576, 4096)), "a megabyte in, four kilobytes out");
    }

    // A kernel without tcp_diag answers the request with an error, and that is
    // not the same as an answer with no sockets in it.
    #[test]
    fn a_refused_dump_is_not_an_empty_one() {
        let mut refused = Vec::new();
        refused.extend_from_slice(&(HEADER as u32).to_ne_bytes());
        refused.extend_from_slice(&NLMSG_ERROR.to_ne_bytes());
        refused.extend_from_slice(&[0u8; 10]);
        let mut out = Carried::new();
        assert!(take_sockets(&refused, &mut out) == End::Refused);
        assert!(out.is_empty());
    }

    #[test]
    fn a_socket_without_counters_is_left_out() {
        let message = vec![0u8; DIAG_MSG];
        assert_eq!(socket_of(&message), None);
        assert_eq!(aligned(17), 20);
    }
}
