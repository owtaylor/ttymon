use crate::process::Process;
use crate::socket::get_socket_peer;
use std::io;
use std::process::Command;

#[derive(Clone)]
pub struct ContainerInfo {
    pub container_id: String,
    pub container_name: String,
    pub image_id: String,
    pub image_name: String,
}

fn have_common_member(a: &[u32], b: &[u32]) -> bool {
    return a.into_iter().any(|v| b.contains(v));
}

pub fn find_podman_peer(tty_pgrp: i32) -> io::Result<(i32, Option<ContainerInfo>)> {
    let pgrp_members = Process::list_process_group(tty_pgrp)?;
    let mut sockets: Vec<u32> = vec![];
    for pid in pgrp_members {
        match Process::new(pid).list_sockets() {
            Ok(s) => {
                let mut new_sockets = s;
                sockets.append(&mut new_sockets);
            }
            Err(e) => {
                println!("Failed to list sockets: {}", e);
            }
        }
    }

    let mut peer_sockets: Vec<u32> = vec![];
    for socket_ino in sockets {
        match get_socket_peer(socket_ino) {
            Ok(peer) => {
                if peer != 0 {
                    peer_sockets.push(peer);
                }
            }
            Err(e) => println!("{}: {:?}", socket_ino, e),
        }
    }

    let conmon_pid = match Process::find(|process: &Process| {
        if let Ok(argv0) = process.argv0() {
            if argv0 == "/usr/bin/conmon" {
                if let Ok(sockets) = process.list_sockets() {
                    return have_common_member(&sockets, &peer_sockets);
                }
            }
        }

        return false;
    }) {
        Ok(Some(process)) => process.pid(),
        Ok(None) => {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Can't find podman peer",
            ))
        }
        Err(e) => return Err(e),
    };

    let container_info = get_container_info(conmon_pid)?;

    return match Process::find(|process: &Process| {
        if let Ok(ppid) = process.parent() {
            ppid == conmon_pid
        } else {
            false
        }
    }) {
        Ok(Some(process)) => Ok((process.pid(), container_info)),
        Ok(None) => {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Can't find podman peer",
            ))
        }
        Err(e) => return Err(e),
    };
}

fn get_container_info_for_id(id: &[u8]) -> io::Result<Option<ContainerInfo>> {
    let container_id = std::string::String::from_utf8(id.to_vec()).unwrap();

    let output = Command::new("podman")
        .arg("inspect")
        .arg(&container_id)
        .arg("-f")
        .arg("{{ .Name }} {{ .Image }} {{ .ImageName }}")
        .output()?;

    if output.status.success() {
        if let Ok(str_output) = String::from_utf8(output.stdout) {
            let fields: Vec<&str> = str_output.split(" ").collect();
            if fields.len() == 3 {
                return Ok(Some(ContainerInfo {
                    container_id: String::from(container_id),
                    container_name: String::from(fields[0]),
                    image_id: String::from(fields[1]),
                    image_name: String::from(fields[2]),
                }));
            }
        }
    }

    return Ok(None);
}

fn get_container_info(conmon_pid: i32) -> io::Result<Option<ContainerInfo>> {
    let process = Process::new(conmon_pid);
    let args = process.cmdline()?;
    let mut arg_iter = args.into_iter();
    loop {
        match arg_iter.next() {
            Some(b"-c") => {
                if let Some(id) = arg_iter.next() {
                    return get_container_info_for_id(id);
                }
            }
            Some(_) => (),
            None => break,
        }
    }

    return Ok(None);
}
