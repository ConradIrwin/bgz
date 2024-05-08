use std::{
    ffi::OsString,
    fs::read_link,
    io,
    os::{
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr, UnixDatagram},
    },
    process,
    thread::sleep,
    time::Duration,
};

use fork::Fork;

// Example of how we might want to DIY the equivalent of macOS's `open`.
// - Spawns the subprocess using fork() + setsid() + exec() [to avoid any potential wierdness caused by running in a forked process]
// - Uses abstract sockets (which are not supported on macOS) to avoid polluting the filesystem, and to get "only one instance per channel" semantics.
// - Uses datagram sockets so we don't need message framing to parse a stream connection
//
// TODO:
// - Decide on serialization format (json?)
// - Wire this into zed...
// - Figure out how to handle errors better...
// - Have `real_main` write to a log file instead of stdout (and probably close stdout once the socket is listening)
//
// https://0xjet.github.io/3OHA/2022/04/11/post.html
// https://man7.org/linux/man-pages/man3/daemon.3.html
// https://github.com/immortal/fork
fn main() -> Result<(), io::Error> {
    let sock_addr = SocketAddr::from_abstract_name("zed-preview")?;

    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1) == Some(&"--for-real".to_string()) {
        real_main(&sock_addr);
        return Ok(());
    }

    let recv_addr = SocketAddr::from_abstract_name(format!("zed-cli-{}", process::id())).unwrap();
    let mut sock = UnixDatagram::bind_addr(&recv_addr)?;
    if sock.connect_addr(&sock_addr).is_err() {
        println!("booting new process");
        boot_background()?;
        wait_for_socket(&sock_addr, &mut sock)?;
    } else {
        println!("connected to running process");
    }

    sock.send(&r#"{"test":1}"#.as_bytes()).unwrap();
    let mut response = [0u8; 1024];
    let len = sock.recv(&mut response)?;
    println!("response: {}", String::from_utf8_lossy(&response[..len]));
    Ok(())
}

fn boot_background() -> Result<(), io::Error> {
    let path = read_link("/proc/self/exe")?;

    match fork::fork() {
        Ok(Fork::Parent(_)) => return Ok(()),
        Ok(Fork::Child) => {
            if let Err(_) = fork::setsid() {
                eprintln!("failed to setsid: {}", std::io::Error::last_os_error());
                process::exit(1);
            }
            let error = exec::execvp(
                path.clone(),
                &[path.as_os_str(), &OsString::from("--for-real")],
            );
            // if exec succeeded, we never get here.
            eprintln!("failed to exec {:?}: {}", path, error);
            process::exit(1);
        }
        Err(_) => {
            return Err(io::Error::last_os_error());
        }
    }
}

fn wait_for_socket(sock_addr: &SocketAddr, sock: &mut UnixDatagram) -> Result<(), std::io::Error> {
    for _ in 0..100 {
        sleep(Duration::from_millis(10));
        if sock.connect_addr(&sock_addr).is_ok() {
            return Ok(());
        }
    }
    sock.connect_addr(&sock_addr)
}

fn real_main(sock_addr: &SocketAddr) {
    let mut buf = [0u8; 1024];
    match UnixDatagram::bind_addr(&sock_addr) {
        Ok(listener) => loop {
            match listener.recv_from(&mut buf) {
                Ok((len, addr)) => {
                    println!(
                        "BG: received: {} from {:?}",
                        String::from_utf8_lossy(&buf[..len]),
                        addr
                    );
                    if let Err(e) = listener.send_to_addr(&"I got it!".as_bytes(), &addr) {
                        eprintln!("BG: failed to send: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("BG: failed to accept: {}", e);
                }
            }
        },
        Err(e) => {
            eprintln!("BG: failed to listen: {}", e);
            process::exit(1);
        }
    }
}
