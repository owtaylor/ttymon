use std::path::Path;
use nix::errno::Errno;
use nix::fcntl::{OFlag, open};
use nix::pty::{grantpt, posix_openpt, ptsname, PtyMaster, unlockpt};
use nix::unistd::{close, dup2, setsid, read, write};
use nix::sys::epoll::{EpollEvent, EpollFlags, EpollOp, epoll_create, epoll_ctl, epoll_wait};
use nix::sys::stat::Mode;
use nix::sys::termios;
use std::io;
use std::cmp::min;
use std::convert::TryInto;
use std::os::unix::io::RawFd;
use std::process::Command;
use std::os::unix::process::CommandExt;
use std::os::unix::io::{AsRawFd};
use std::time::{Duration, Instant};

use crate::filter::Filter;

// Check at .1 / .5 / 2.5 / 12.5 / .... / 60 seconds
const MIN_CHECK_INTERVAL: std::time::Duration = Duration::from_millis(100);
const MAX_CHECK_INTERVAL: std::time::Duration = Duration::from_secs(60);
const CHECK_INTERVAL_MULTIPLIER: u32 = 5;

const STDIN: RawFd = 0;
const STDOUT: RawFd = 1;

struct RawInput {
    orig_attr: termios::Termios
}

impl RawInput {
    fn setup() -> nix::Result<RawInput> {
        let orig_attr = termios::tcgetattr(0)?;
        let mut new_attr = orig_attr.clone();
        termios::cfmakeraw(&mut new_attr);
        termios::tcsetattr(0, termios::SetArg::TCSAFLUSH, &new_attr)?;

        Ok(RawInput{ orig_attr })
    }
}

impl Drop for RawInput {
    fn drop(&mut self) {
        if let Err(e) = termios::tcsetattr(0, termios::SetArg::TCSAFLUSH, &self.orig_attr) {
            println!("Can't restore terminal settings: {}", e);
        }
    }
}

fn write_all(fd: RawFd, buf: &[u8]) -> nix::Result<()> {
    let mut written = 0;
    while written < buf.len() {
        match write(fd, &buf[written..]) {
            Ok(write_count) => written += write_count,
            Err(nix::Error::Sys(Errno::EINTR)) => {},
            Err(e) => { return Err(e) },
        }
    }

    Ok(())
}

struct Buffer {
    buf: Vec<u8>,
    count: usize,
}

impl Buffer {
    fn new() -> Self {
        return Buffer { buf: vec![0; 4096], count: 0 };
    }

    fn fill(&mut self, fd: RawFd) -> nix::Result<bool> {
        match read(fd, &mut self.buf[self.count..]) {
            Ok(0) => { Ok(false) },
            Ok(count) => {
                self.count += count;
                Ok(true)
            },
            Err(e) => Err(e),
        }
    }

    fn flush(&mut self, fd: RawFd) -> nix::Result<()> {
        write_all(fd, &self.buf[0..self.count])?;
        self.count = 0;
        Ok(())
    }
}

struct FilteredBuffer {
    raw: Buffer,
    filter: Filter,
}

impl FilteredBuffer {
    fn new() -> Self {
        return FilteredBuffer { raw: Buffer::new(), filter: Filter::new() };
    }

    fn fill(&mut self, fd: RawFd) -> nix::Result<bool> {
        if !self.raw.fill(fd)? {
            return Ok(false);
        }

        self.filter.fill(&self.raw.buf[0..self.raw.count]);
        self.raw.count = 0;
        Ok(true)
    }

    fn flush(&mut self, fd: RawFd) -> nix::Result<()> {
        {
            let buf = self.filter.buffer();
            write_all(fd, buf)?;
        }
        self.filter.clear_buffer();
        Ok(())
    }
}

pub struct Pty {
    master_fd: PtyMaster,
    peer_fd: RawFd,
    check_interval: Duration,
    last_check_time: Option<Instant>,
}

impl Pty {
    pub fn new() -> nix::Result<Pty> {
        // Open a new PTY master
        let master_fd = posix_openpt(OFlag::O_RDWR)?;

        // Allow a slave to be generated for it
        grantpt(&master_fd)?;
        unlockpt(&master_fd)?;

        // Get the name of the slave
        let peer_name = unsafe { ptsname(&master_fd) }?;

        // Try to open the slave
        let peer_fd = open(Path::new(&peer_name), OFlag::O_RDWR, Mode::empty())?;

        Ok(Pty { master_fd, peer_fd, check_interval: MIN_CHECK_INTERVAL, last_check_time: None })
    }

    fn child_setup(peer_fd: RawFd) -> nix::Result<()> {
        dup2(peer_fd, 0)?;
        dup2(peer_fd, 1)?;
        dup2(peer_fd, 2)?;

        setsid()?;

        Ok(())
    }

    fn close_peer_fd(&mut self) -> nix::Result<()> {
        if self.peer_fd != -1 {
            let res = close(self.peer_fd);
            self.peer_fd = -1;
            res
        } else {
            Ok(())
        }
    }

    pub fn fork(&mut self) -> io::Result<u32> {
        let mut proc = Command::new("/bin/bash");

        let peer_fd = self.peer_fd;
        unsafe {
            proc.pre_exec(move || {
                match Self::child_setup(peer_fd) {
                    Ok(()) => Ok(()),
                    Err(nix::Error::Sys(e)) => return Err(e.into()),
                    Err(e) => return Err(io::Error::new(io::ErrorKind::Other, format!("Spawn failed: {}", e))),
                }
            });
        }

        let child = proc.spawn()?;
        self.close_peer_fd().unwrap();

        Ok(child.id())
    }

    fn maybe_check<A>(&mut self, actions: &mut A, from_child: &mut FilteredBuffer) -> Duration where A: PtyActions {
        let now = Instant::now();
        let next_check_time = if let Some(last_check_time) = self.last_check_time {
            last_check_time + self.check_interval
        } else {
            now
        };

        if next_check_time <= now {
            actions.check();

            let in_window_title = from_child.filter.in_window_title();
            let out_window_title = actions.make_window_title(in_window_title);
            from_child.filter.set_out_window_title(&out_window_title);
            let _ = from_child.flush(STDOUT);

            self.check_interval = min(MAX_CHECK_INTERVAL,
                                      self.check_interval * CHECK_INTERVAL_MULTIPLIER);
            self.last_check_time = Some(now);
            self.check_interval
        } else {
            next_check_time - now
        }
    }

    pub fn handle<A>(&mut self, actions: &mut A) -> nix::Result<()> where A: PtyActions {
        let raw_input = RawInput::setup();
        if let Err(e) = raw_input {
            println!("Can't setup raw input: {}", e);
        };

        let master_fd = self.master_fd.as_raw_fd();

        let epoll_fd = epoll_create()?;

        let mut from_child = FilteredBuffer::new();
        let mut to_child = Buffer::new();

        let mut event = EpollEvent::new(EpollFlags::EPOLLIN, 0);
        epoll_ctl(epoll_fd, EpollOp::EpollCtlAdd, master_fd,  &mut event)?;
        let mut event = EpollEvent::new(EpollFlags::EPOLLIN, 1);
        epoll_ctl(epoll_fd, EpollOp::EpollCtlAdd, STDIN, &mut event)?;

        let mut events = vec![EpollEvent::empty(), EpollEvent::empty()];
        let mut done = false;
        while !done {
            let remaining = self.maybe_check(actions, &mut from_child);

            let event_count = epoll_wait(epoll_fd, &mut events, remaining.as_millis().try_into().unwrap())?;
            for event in &events[0..event_count] {
                match event.data() {
                    0 => {
                        if event.events().contains(EpollFlags::EPOLLIN) ||
                               event.events().contains(EpollFlags::EPOLLHUP) {
                            if from_child.fill(master_fd)? {
                                from_child.flush(STDOUT)?;
                                self.check_interval = MIN_CHECK_INTERVAL;
                            } else {
                                done = true;
                            }
                        }
                    },
                    1 => {
                        if event.events().contains(EpollFlags::EPOLLIN) ||
                               event.events().contains(EpollFlags::EPOLLHUP) {
                            if to_child.fill(STDIN)? {
                                to_child.flush(master_fd)?;
                            } else {
                                done = true;
                            }
                        }
                    },
                    _ => ()
                }
            }
        }

        return Ok(());
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.close_peer_fd().unwrap();
    }
}

pub trait PtyActions {
    fn check(&mut self);
    fn make_window_title(&self, in_window_title: &str) -> String {
        return in_window_title.to_string();
    }
}
