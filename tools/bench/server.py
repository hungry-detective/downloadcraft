"""Local test server for the benchmark: files with Range support.

Paths: /<name>?rate=<bytes per second per connection>&drop=<bytes before
the server cuts the connection>. Both optional.
"""
import os, sys, time, threading
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs

ROOT = sys.argv[1]
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 8765
CHUNK = 64 * 1024


SEEN = set()
LOCK = threading.Lock()
LOG = os.path.join(os.path.dirname(os.path.abspath(ROOT)), "requests.log")


def stamp(name):
    with LOCK:
        if name in SEEN:
            return
        SEEN.add(name)
        with open(LOG, "a") as f:
            f.write(f"{name} {time.time():.4f}\n")


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _serve(self, body):
        u = urlparse(self.path)
        # The first request for each name, for the benchmark's clock: every
        # program is timed from the moment it first asks the server.
        stamp(os.path.basename(u.path))
        q = parse_qs(u.query)
        rate = int(q.get("rate", ["0"])[0])
        drop = int(q.get("drop", ["0"])[0])
        name = os.path.basename(u.path)
        # "file1g__run3.bin" serves file1g.bin: a new name for every run.
        stem, ext = os.path.splitext(name)
        path = os.path.join(ROOT, stem.split("__")[0] + ext)
        if not os.path.isfile(path):
            self.send_error(404)
            return
        size = os.path.getsize(path)
        start, end = 0, size - 1
        rng = self.headers.get("Range")
        status = 200
        if rng and rng.startswith("bytes="):
            a, _, b = rng[6:].split(",")[0].partition("-")
            if a:
                start = int(a)
                end = int(b) if b else size - 1
            else:
                start = size - int(b)
            end = min(end, size - 1)
            status = 206
        length = end - start + 1
        self.send_response(status)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(length))
        self.send_header("Last-Modified", "Mon, 02 Nov 2026 10:00:00 GMT")
        self.send_header("ETag", '"' + name + '"')
        if status == 206:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        if not body:
            return
        sent = 0
        t0 = time.perf_counter()
        with open(path, "rb") as f:
            f.seek(start)
            while sent < length:
                n = min(CHUNK, length - sent)
                if drop and sent + n > drop:
                    self.close_connection = True
                    try:
                        self.wfile.write(f.read(max(0, drop - sent)))
                    except OSError:
                        pass
                    return
                try:
                    self.wfile.write(f.read(n))
                except OSError:
                    return
                sent += n
                if rate:
                    ahead = sent / rate - (time.perf_counter() - t0)
                    if ahead > 0:
                        time.sleep(ahead)

    def do_GET(self):
        self._serve(True)

    def do_HEAD(self):
        self._serve(False)


ThreadingHTTPServer.daemon_threads = True
print(f"ready {PORT}", flush=True)
ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
