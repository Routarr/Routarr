//! systemd's readiness protocol: a unit of `Type=notify` learns from these datagrams when
//! Routarr serves and when it starts to stop. Elsewhere `NOTIFY_SOCKET` is unset and nothing
//! is sent.

use std::ffi::OsStr;

/// Tell the service manager `state`, `READY=1` or `STOPPING=1`, when it listens. A failure
/// is logged and changes nothing: the manager then judges the start by its own timeout.
pub fn notify(state: &str) {
    let Some(socket) = std::env::var_os("NOTIFY_SOCKET") else { return };
    if let Err(e) = send(&socket, state) {
        tracing::warn!("The service manager could not be told {state}: {e}");
    }
}

/// Send `state` to `socket`, a path or, after `@`, a name in Linux's abstract namespace.
#[cfg(unix)]
fn send(socket: &OsStr, state: &str) -> std::io::Result<()> {
    let datagram = std::os::unix::net::UnixDatagram::unbound()?;
    if let Some(name) = socket.as_encoded_bytes().strip_prefix(b"@") {
        return send_abstract(&datagram, name, state);
    }
    datagram.send_to(state.as_bytes(), std::path::Path::new(socket)).map(|_| ())
}

#[cfg(target_os = "linux")]
fn send_abstract(
    datagram: &std::os::unix::net::UnixDatagram,
    name: &[u8],
    state: &str,
) -> std::io::Result<()> {
    use std::os::linux::net::SocketAddrExt;
    let address = std::os::unix::net::SocketAddr::from_abstract_name(name)?;
    datagram.send_to_addr(state.as_bytes(), &address).map(|_| ())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn send_abstract(_: &std::os::unix::net::UnixDatagram, _: &[u8], _: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "an abstract socket exists on Linux alone",
    ))
}

#[cfg(not(unix))]
fn send(_: &OsStr, _: &str) -> std::io::Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixDatagram;

    fn received(listener: &UnixDatagram) -> String {
        listener.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        let mut buffer = [0u8; 64];
        let length = listener.recv(&mut buffer).unwrap();
        String::from_utf8_lossy(&buffer[..length]).into_owned()
    }

    #[test]
    fn the_state_reaches_a_socket_named_by_its_path() {
        let dir = std::env::temp_dir().join(format!("routarr-notify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notify.sock");
        std::fs::remove_file(&path).ok();
        let listener = UnixDatagram::bind(&path).unwrap();

        send(path.as_os_str(), "READY=1").unwrap();
        assert_eq!(received(&listener), "READY=1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_state_reaches_a_socket_of_the_abstract_namespace() {
        use std::os::linux::net::SocketAddrExt;
        let name = format!("routarr-notify-{}", std::process::id());
        let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let listener = UnixDatagram::bind_addr(&address).unwrap();

        send(OsStr::new(&format!("@{name}")), "STOPPING=1").unwrap();
        assert_eq!(received(&listener), "STOPPING=1");
    }

    #[test]
    fn a_socket_nobody_listens_on_is_an_error_not_a_panic() {
        let path = std::env::temp_dir().join(format!("routarr-no-socket-{}", std::process::id()));
        assert!(send(path.as_os_str(), "READY=1").is_err());
    }
}
