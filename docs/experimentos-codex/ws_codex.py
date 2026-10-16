"""Cliente WebSocket minimo (stdlib) para o socket de controle do app-server do Codex. SOMENTE caminho "/" (nunca /daemon/shutdown)."""
import base64, json, os, socket, struct, time, select, sys
SOCK = os.path.expanduser("~/.codex/app-server-control/app-server-control.sock")
class WS:
    def __init__(self):
        self.s = socket.socket(socket.AF_UNIX); self.s.settimeout(8); self.s.connect(SOCK)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall((f"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        buf = b""
        while b"\r\n\r\n" not in buf: buf += self.s.recv(4096)
        head, self.buf = buf.split(b"\r\n\r\n", 1)
        if b" 101 " not in head.split(b"\r\n")[0]: raise SystemExit("handshake falhou: " + head.decode()[:120])
    def send(self, obj):
        data = json.dumps(obj).encode(); mask = os.urandom(4); n = len(data)
        hdr = bytes([0x81]) + (bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n) if n < 65536 else bytes([0x80 | 127]) + struct.pack(">Q", n))
        self.s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))
    def _need(self, n):
        while len(self.buf) < n: self.buf += self.s.recv(65536)
    def recv(self):
        self._need(2); b0, b1 = self.buf[0], self.buf[1]; n = b1 & 0x7f; off = 2
        if n == 126: self._need(4); n = struct.unpack(">H", self.buf[2:4])[0]; off = 4
        elif n == 127: self._need(10); n = struct.unpack(">Q", self.buf[2:10])[0]; off = 10
        self._need(off + n); payload = self.buf[off:off + n]; self.buf = self.buf[off + n:]
        op = b0 & 0x0f
        if op == 1: return json.loads(payload)
        if op == 8: return None
        return {"_op": op}
    def call(self, id_, method, params):
        self.send({"id": id_, "method": method, "params": params})
        end = time.time() + 10
        while time.time() < end:
            m = self.recv()
            if m is None: return None
            if m.get("id") == id_: return m
    def close(self):
        try: self.s.sendall(bytes([0x88, 0x80]) + os.urandom(4))
        except OSError: pass
        self.s.close()
