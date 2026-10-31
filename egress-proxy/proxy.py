"""Egress sidecar: an HTTP CONNECT proxy whose outbound connections are bound
to one network interface (the host's VPN, eth1).

Why it exists: the VPN interface lives on the host, not inside the webserver
containers, so the Movies proxy (routes/movies.rs) sends its curl traffic here
and this process, running with host networking, dials out over that interface
with SO_BINDTODEVICE (the same pinning `curl --interface` does).

Locked down on purpose, since it listens on the host:
  - only clients from private/loopback addresses (the Docker networks) are served;
  - only CONNECT, only to ALLOWED_HOSTS on port 443;
  - if the interface is missing it answers 503 and never falls back to the
    normal route.

Environment: EGRESS_INTERFACE (default eth1), LISTEN_HOST (0.0.0.0),
LISTEN_PORT (3128), ALLOWED_HOSTS (comma-separated, default below).
"""
import asyncio
import ipaddress
import os
import socket

INTERFACE = os.environ.get("EGRESS_INTERFACE", "eth1").strip() or "eth1"
LISTEN_HOST = os.environ.get("LISTEN_HOST", "0.0.0.0")
LISTEN_PORT = int(os.environ.get("LISTEN_PORT", "3128"))
ALLOWED_HOSTS = {
    h.strip().lower()
    for h in os.environ.get("ALLOWED_HOSTS", "cno.to,v.cno.to,api.theintrodb.org").split(",")
    if h.strip()
}
CONNECT_TIMEOUT = 10
IDLE_TIMEOUT = 120


def log(msg):
    print(f"[egress] {msg}", flush=True)


def interface_exists():
    return os.path.isdir(f"/sys/class/net/{INTERFACE}")


def client_allowed(peer):
    try:
        ip = ipaddress.ip_address(peer)
    except ValueError:
        return False
    return ip.is_private or ip.is_loopback


async def reply(writer, status):
    writer.write(f"HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".encode())
    try:
        await writer.drain()
    except OSError:
        pass


async def dial(host):
    """Connect to host:443 out of INTERFACE only."""
    loop = asyncio.get_running_loop()
    infos = await loop.getaddrinfo(host, 443, type=socket.SOCK_STREAM)
    last = OSError("no addresses")
    for family, type_, proto, _, addr in infos:
        sock = socket.socket(family, type_, proto)
        try:
            sock.setblocking(False)
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_BINDTODEVICE, INTERFACE.encode())
            await asyncio.wait_for(loop.sock_connect(sock, addr), CONNECT_TIMEOUT)
            return await asyncio.open_connection(sock=sock)
        except (OSError, asyncio.TimeoutError) as err:
            sock.close()
            last = err
    raise last


async def pipe(reader, writer):
    try:
        while True:
            data = await asyncio.wait_for(reader.read(65536), IDLE_TIMEOUT)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except (OSError, asyncio.TimeoutError, asyncio.IncompleteReadError):
        pass
    finally:
        try:
            writer.close()
        except OSError:
            pass


async def handle(reader, writer):
    peer = (writer.get_extra_info("peername") or ("?",))[0]
    try:
        if not client_allowed(peer):
            return
        try:
            head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 10)
        except (asyncio.TimeoutError, asyncio.IncompleteReadError, asyncio.LimitOverrunError):
            return
        parts = head.split(b"\r\n", 1)[0].decode("latin-1").split()
        if len(parts) != 3 or parts[0] != "CONNECT":
            return await reply(writer, "405 Method Not Allowed")
        host, _, port = parts[1].rpartition(":")
        host = host.lower()
        if port != "443" or host not in ALLOWED_HOSTS:
            log(f"refused {parts[1]} from {peer}")
            return await reply(writer, "403 Forbidden")
        if not interface_exists():
            log(f"interface {INTERFACE!r} is missing; refusing to use the normal route")
            return await reply(writer, "503 Service Unavailable")
        try:
            up_reader, up_writer = await dial(host)
        except (OSError, asyncio.TimeoutError) as err:
            log(f"connect to {host} via {INTERFACE} failed: {err}")
            return await reply(writer, "502 Bad Gateway")
        writer.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        await writer.drain()
        await asyncio.gather(pipe(reader, up_writer), pipe(up_reader, writer))
    except OSError:
        pass
    finally:
        try:
            writer.close()
        except OSError:
            pass


async def main():
    # Don't exit when the interface is missing: a crash-looping sidecar would
    # block deploys that depend on it. Requests get 503 until it appears.
    if not interface_exists():
        log(f"WARNING: interface {INTERFACE!r} not found on this host; answering 503 until it exists")
    server = await asyncio.start_server(handle, LISTEN_HOST, LISTEN_PORT)
    log(f"listening on {LISTEN_HOST}:{LISTEN_PORT}, out via {INTERFACE}, hosts: {', '.join(sorted(ALLOWED_HOSTS))}")
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
