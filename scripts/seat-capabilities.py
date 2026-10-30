# Prints what the TV's compositor (cage) tells programs about its input seat:
# whether it has a pointer, a keyboard, touch. Only reads. See README.md.
import glob, os, socket, struct

def connect():
    run = os.environ["XDG_RUNTIME_DIR"]
    for path in sorted(glob.glob(os.path.join(run, "wayland-[0-9]"))):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            s.connect(path)
            return s, path
        except OSError:
            s.close()
    raise SystemExit("no Wayland socket in " + run)

def msg(obj, op, payload=b""):
    return struct.pack("<II", obj, ((8 + len(payload)) << 16) | op) + payload

def string(text):
    b = text.encode() + b"\0"
    return struct.pack("<I", len(b)) + b + b"\0" * (-len(b) % 4)

def events(sock, until_callback):
    buf = b""
    while True:
        while len(buf) >= 8:
            obj, so = struct.unpack("<II", buf[:8])
            size, op = so >> 16, so & 0xFFFF
            if len(buf) < size:
                break
            body, buf = buf[8:size], buf[size:]
            yield obj, op, body
            if obj == until_callback:
                return
        data = sock.recv(65536)
        if not data:
            return
        buf += data

def read_string(body, off):
    n = struct.unpack_from("<I", body, off)[0]
    text = body[off + 4:off + 4 + n - 1].decode()
    return text, off + 4 + n + (-n % 4)

sock, path = connect()
sock.sendall(msg(1, 1, struct.pack("<I", 2)) + msg(1, 0, struct.pack("<I", 3)))  # get_registry, sync
seats = []
for obj, op, body in events(sock, 3):
    if obj == 2 and op == 0:
        name = struct.unpack_from("<I", body, 0)[0]
        iface, off = read_string(body, 4)
        version = struct.unpack_from("<I", body, off)[0]
        if iface == "wl_seat":
            seats.append((name, version))
print("socket:", path)
next_id = 4
for name, version in seats:
    v = min(version, 5)
    seat_id = next_id
    sock.sendall(msg(2, 0, struct.pack("<I", name) + string("wl_seat") + struct.pack("<II", v, seat_id))
                 + msg(1, 0, struct.pack("<I", seat_id + 1)))
    for obj, op, body in events(sock, seat_id + 1):
        if obj == seat_id and op == 0:
            caps = struct.unpack_from("<I", body, 0)[0]
            parts = [n for bit, n in ((1, "pointer"), (2, "keyboard"), (4, "touch")) if caps & bit]
            print(f"seat capabilities: {caps} ({', '.join(parts) or 'none'})")
        elif obj == seat_id and op == 1:
            print("seat name:", read_string(body, 0)[0])
    next_id += 2
