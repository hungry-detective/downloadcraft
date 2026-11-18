"""Download Craft, IDM and one connection (curl) on the same PC, the same
local test server and the same files, all timed the same way: from the
first request the program sends to the server (the server notes it) until
the whole file is on disk.

    python server.py files 8765     (in another window)
    python bench_all.py             (writes results.json)

Download Craft and curl run by themselves. For IDM this prints
"READY <link>": open that link in Chrome so IDM's extension takes it (we
used Chrome DevTools); a watcher presses IDM's Start Download when its
window opens. Each case runs RUNS times (3); the middle run counts. Every
file up to 300 MB is checked byte for byte against the original.
"""
import hashlib, json, os, random, statistics, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
FILES = os.path.join(HERE, "files")
OUT = os.path.join(HERE, "out")
LOG = os.path.join(HERE, "requests.log")
DC = os.environ.get("DC_EXE", os.path.join(HERE, "..", "..", "target", "release", "download_craft.exe"))
CLICK = os.path.join(HERE, "idm_click.ps1")
DL = os.path.join(os.environ.get("USERPROFILE", HERE), "Downloads")
BASE = "http://127.0.0.1:8765/"
RUNS = int(os.environ.get("RUNS", "3"))
APPS = (os.environ.get("APPS") or "dc,one,idm").split(",")
CASES = [
    ("big", "file1g", "", "1 GB file"),
    ("capped", "file300m", "?rate=4194304", "300 MB, each connection capped at 4 MB/s"),
    ("small", "file1m", "", "1 MB file"),
    ("drops", "file300m", "?drop=20971520", "300 MB, connections cut every 20 MB"),
]


def make_files():
    os.makedirs(FILES, exist_ok=True)
    for name, size in [("file1g.bin", 1 << 30), ("file300m.bin", 300 << 20), ("file1m.bin", 1 << 20)]:
        p = os.path.join(FILES, name)
        if os.path.exists(p) and os.path.getsize(p) == size:
            continue
        with open(p, "wb") as f:
            left = size
            while left:
                n = min(left, 8 << 20)
                f.write(os.urandom(n))
                left -= n


def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def first_request(name, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        try:
            for line in open(LOG):
                n, t = line.split()
                if n == name:
                    return float(t)
        except OSError:
            pass
        time.sleep(0.05)
    return None


def wait_size(path, size, timeout=900):
    """When the file at `path` is whole: its full size, and let go of by the
    program writing it (IDM moves the file from its own temporary folder
    into place; a moved file has its full size before all of it is
    written). Renaming a file succeeds only when no program holds it."""
    end = time.time() + timeout
    while time.time() < end:
        try:
            if os.path.getsize(path) == size:
                os.rename(path, path + ".chk")
                os.rename(path + ".chk", path)
                return time.time()
        except OSError:
            pass
        time.sleep(0.005)
    return None


def run_dc(url, dest, size):
    # Done when the finished file appears under its name (Download Craft
    # writes a working file and renames it when complete), as for IDM; not
    # when the program has also shut down.
    p = subprocess.Popen([DC, "get", url, "-o", dest, "-c", "8"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    done = None
    while done is None and p.poll() is None:
        try:
            if os.path.getsize(dest) == size:
                # The same test as for IDM: whole, and let go of.
                os.rename(dest, dest + ".chk")
                os.rename(dest + ".chk", dest)
                done = time.time()
        except OSError:
            pass
        time.sleep(0.002)
    p.wait()
    if done is None and os.path.exists(dest) and os.path.getsize(dest) == size:
        done = time.time()
    return done


def run_one(url, dest, size):
    for _ in range(100):
        subprocess.run(["curl.exe", "-s", "-C", "-", "-o", dest, url], capture_output=True, timeout=900)
        if os.path.exists(dest) and os.path.getsize(dest) == size:
            return time.time()
    return None


def run_idm(url, dest, size):
    w = subprocess.Popen(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", CLICK,
                          "-Title", "Download File Info", "-Button", "Start Download", "-WaitMs", "180000"],
                         stdout=subprocess.PIPE, text=True)
    time.sleep(2.5)  # the watcher is ready
    print("READY", url, flush=True)
    if "pressed" not in w.communicate()[0]:
        return None
    done = wait_size(dest, size)
    subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", CLICK,
                    "-Title", "Download complete", "-Button", "Close", "-WaitMs", "4000"], capture_output=True)
    return done


RUN = {"dc": run_dc, "one": run_one, "idm": run_idm}


def main():
    make_files()
    os.makedirs(OUT, exist_ok=True)
    res_path = os.path.join(HERE, "results.json")
    R = json.load(open(res_path)) if os.path.exists(res_path) else {}
    for key, stem, query, label in CASES:
        src = os.path.join(FILES, stem + ".bin")
        size = os.path.getsize(src)
        want = sha(src) if size <= (300 << 20) else None
        row = R.setdefault(key, {})
        row["label"] = label
        for app in APPS:
            times = []
            for i in range(RUNS):
                name = f"{stem}__{app}{key}{i}{random.randint(1000, 9999)}.bin"
                dest = os.path.join(DL if app == "idm" else OUT, name)
                done = RUN[app](BASE + name + query, dest, size)
                start = first_request(name)
                ok = done is not None and start is not None and (want is None or sha(dest) == want)
                times.append(round(done - start, 3) if ok else None)
                print(key, app, i, times[-1], flush=True)
                time.sleep(1.0)
                try:
                    os.remove(dest)
                except OSError:
                    pass
            good = [x for x in times if x is not None]
            row[app] = times
            row[app + "_median"] = statistics.median(good) if good else None
            json.dump(R, open(res_path, "w"), indent=2)
    print(json.dumps({k: {a: v.get(a + "_median") for a in APPS} for k, v in R.items()}), flush=True)


if __name__ == "__main__":
    main()
