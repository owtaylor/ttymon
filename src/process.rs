use regex::Regex;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::str::FromStr;

lazy_static! {
    static ref ALL_NUMBERS_RE: Regex = Regex::new(r"^\d+$").unwrap();
    static ref SOCKET_RE: Regex = Regex::new(r"^socket:\[(\d+)\]$").unwrap();
}

#[derive(Debug)]
pub struct Process {
    pid: i32,
    proc_path: std::path::PathBuf,
}

struct ProcessIterator {
    read_dir: fs::ReadDir,
}

impl ProcessIterator {
    fn new() -> io::Result<ProcessIterator> {
        Ok(ProcessIterator {
            read_dir: fs::read_dir(Path::new("/proc"))?,
        })
    }
}

impl Iterator for ProcessIterator {
    type Item = io::Result<Process>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = match self.read_dir.next() {
                Some(Ok(x)) => x,
                Some(Err(e)) => return Some(Err(e)),
                None => {
                    return None;
                }
            };

            if let Some(file_name) = entry.file_name().to_str() {
                if ALL_NUMBERS_RE.is_match(file_name) {
                    return Some(Ok(Process {
                        pid: file_name.parse().unwrap(),
                        proc_path: entry.path(),
                    }));
                }
            }
        }
    }
}

pub struct Args(Vec<u8>);

impl<'a> IntoIterator for &'a Args {
    type Item = &'a [u8];
    type IntoIter = std::slice::Split<'a, u8, fn(&u8) -> bool>;

    fn into_iter(self) -> std::slice::Split<'a, u8, fn(&u8) -> bool> {
        self.0.split(|x| *x == 0)
    }
}

struct StatParser(Vec<u8>);

impl StatParser {
    fn new(proc_path: &Path) -> io::Result<StatParser> {
        let mut f = fs::File::open(proc_path.join("stat"))?;

        let mut s = StatParser(vec![]);
        f.read_to_end(&mut s.0)?;

        return Ok(s);
    }

    fn parse<'a>(&'a self) -> io::Result<Vec<&'a [u8]>> {
        const NOT_FOUND: usize = (-1isize) as usize;

        let mut open_paren = NOT_FOUND;
        for i in 1..self.0.len() {
            if self.0[i - 1] == 0x20 && self.0[i] == 0x28 {
                open_paren = i;
                break;
            }
        }
        let mut close_paren = NOT_FOUND;
        for i in (0..self.0.len() - 1).rev() {
            if self.0[i] == 0x29 && self.0[i + 1] == 0x20 {
                close_paren = i;
                break;
            }
        }

        if open_paren != NOT_FOUND && close_paren != NOT_FOUND {
            let mut fields: Vec<&'a [u8]> = vec![];

            fields.push(&self.0[0..open_paren - 1]);
            fields.push(&self.0[open_paren + 1..close_paren]);
            let mut remaining: Vec<&'a [u8]> =
                self.0[close_paren + 2..].split(|x| *x == 0x20).collect();
            fields.append(&mut remaining);

            Ok(fields)
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "Can't parse /proc/stat",
            ))
        }
    }
}

impl Process {
    pub fn new(pid: i32) -> Self {
        Process {
            pid: pid,
            proc_path: Path::new("/proc").join(pid.to_string()),
        }
    }

    pub fn find<P>(pred: P) -> io::Result<Option<Process>>
    where
        P: Fn(&Process) -> bool,
    {
        for process in ProcessIterator::new()? {
            let process = process?;
            if pred(&process) {
                return Ok(Some(process));
            }
        }

        return Ok(None);
    }

    pub fn list_process_group(pgrp: i32) -> io::Result<Vec<i32>> {
        let mut result: Vec<i32> = vec![];

        for process in ProcessIterator::new()? {
            let process = process?;
            if let Ok(process_pgrp) = process.process_group() {
                if process_pgrp == pgrp {
                    result.push(process.pid);
                }
            }
        }

        return Ok(result);
    }

    pub fn cmdline(&self) -> io::Result<Args> {
        let cmdline = self.proc_path.join("cmdline");
        let mut f = fs::File::open(cmdline)?;

        let mut args = Args(Vec::new());
        f.read_to_end(&mut args.0)?;

        return Ok(args);
    }

    pub fn argv0(&self) -> io::Result<String> {
        let args = self.cmdline()?;
        let first = args.into_iter().next().unwrap();
        return if let Ok(first_str) = std::str::from_utf8(first) {
            Ok(first_str.to_string())
        } else {
            Ok("???".to_string())
        };
    }

    pub fn list_sockets(&self) -> io::Result<Vec<u32>> {
        let mut result = Vec::new();

        for entry in fs::read_dir(self.proc_path.join("fd"))? {
            let entry = entry?;
            let link = fs::read_link(entry.path())?;
            if let Some(link_str) = link.to_str() {
                if let Some(captures) = SOCKET_RE.captures(link_str) {
                    let socket: u32 = captures.get(1).unwrap().as_str().parse().unwrap();
                    result.push(socket);
                }
            }
        }

        return Ok(result);
    }

    fn get_stat_field<T: FromStr>(&self, index: usize, name: &str) -> io::Result<T> {
        let stat_parser = StatParser::new(&self.proc_path)?;
        let fields = stat_parser.parse()?;

        if fields.len() > index {
            if let Ok(ppid_field) = std::str::from_utf8(fields[index]) {
                if let Ok(ppid) = ppid_field.parse() {
                    return Ok(ppid);
                }
            }
        }

        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Can't parse {} from /proc/stat", name),
        ));
    }

    pub fn parent(&self) -> io::Result<i32> {
        self.get_stat_field(3, "ppid")
    }

    pub fn process_group(&self) -> io::Result<i32> {
        self.get_stat_field(4, "pgrp")
    }

    pub fn tty_process_group(&self) -> io::Result<i32> {
        self.get_stat_field(7, "tty_pgrp")
    }

    pub fn cwd(&self) -> io::Result<PathBuf> {
        fs::read_link(self.proc_path.join("cwd"))
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }
}
