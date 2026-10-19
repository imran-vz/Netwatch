#!/usr/bin/env python3
"""Read-only macOS connection monitor. Python 3 standard library only.

Run: python3 scripts/monitor-connection.py
Logs: ~/Library/Logs/NetwatchDiagnostics/connection.jsonl (rotated, <=25 MiB)
Stop: Ctrl-C, or send SIGTERM to the PID printed at startup.
Logs contain local IP addresses, DNS/VPN configuration and possibly Wi-Fi names;
review before sharing publicly. No packet capture, browser history or credentials.
"""
import argparse
import concurrent.futures
import datetime
import fcntl
import json
import ipaddress
import socket
import logging
import logging.handlers
import os
from pathlib import Path
import re
import signal
import shutil
import subprocess
import sys
import threading
import time


def command(args, timeout=6, max_output=20000):
    start = time.monotonic()
    try:
        result = subprocess.run(args, capture_output=True, text=True, errors="replace", timeout=timeout)
        return {"ok": result.returncode == 0, "exit": result.returncode,
                "elapsed_ms": round((time.monotonic() - start) * 1000),
                "output": (result.stdout + result.stderr).strip()[:max_output]}
    except (subprocess.TimeoutExpired, OSError) as exc:
        return {"ok": False, "elapsed_ms": round((time.monotonic() - start) * 1000),
                "error": "timeout" if isinstance(exc, subprocess.TimeoutExpired) else str(exc)}


def route():
    result = command(["/sbin/route", "-n", "get", "default"])
    text = result.get("output", "")
    for key in ("gateway", "interface"):
        match = re.search(r"^\s*" + key + r":\s*(\S+)", text, re.M)
        result[key] = match.group(1) if match else None
    return result


def ping(address):
    if not address:
        return {"ok": False, "error": "no default gateway"}
    return command(["/sbin/ping", "-n", "-c", "2", "-i", "0.5", "-W", "1500", address], 5)


def https(direct=False, ipv6=False):
    args = ["/usr/bin/curl", "--noproxy", "*", "-6" if ipv6 else "-4", "-sS", "-o", "/dev/null",
            "--connect-timeout", "3", "--max-time", "6", "-w",
            "%{http_code} %{time_namelookup} %{time_connect} %{time_appconnect} %{time_total}"]
    if direct:
        # Validated TLS to a fixed IP: does not depend on DNS resolution.
        args += ["--resolve", "one.one.one.one:443:1.1.1.1", "https://one.one.one.one/cdn-cgi/trace"]
    else:
        args += ["https://www.apple.com/library/test/success.html"]
    result = command(args, 8)
    fields = result.get("output", "").split()
    if len(fields) >= 5 and fields[0].isdigit():
        try:
            result.update(http_status=int(fields[0]), dns_s=float(fields[1]),
                          connect_s=float(fields[2]), tls_s=float(fields[3]), total_s=float(fields[4]))
            result["ok"] = result["ok"] and 200 <= result["http_status"] < 400
        except ValueError:
            pass
    return result


def local_tcp(address):
    if not address:
        return {"ok": False, "error": "no address"}
    start = time.monotonic()
    try:
        with socket.create_connection((address, 80), timeout=2):
            pass
        return {"ok": True, "elapsed_ms": round((time.monotonic() - start) * 1000)}
    except OSError as exc:
        return {"ok": False, "elapsed_ms": round((time.monotonic() - start) * 1000),
                "error": str(exc)}


def tailscale_backend():
    cli = shutil.which("tailscale")
    if not cli:
        return {"available": False}
    result = command([cli, "status", "--json"], 6, 1000000)
    try:
        data = json.loads(result.get("output", ""))
        return {"available": True, "ok": result["ok"], "backend_state": data.get("BackendState"),
                "exit_node_selected": data.get("ExitNodeStatus") is not None}
    except ValueError:
        return {"available": True, "ok": False, "error": result.get("error", "status unavailable")}


def wifi_events():
    result = command(["/usr/bin/log", "show", "--last", "90s", "--style", "ndjson", "--info",
                      "--predicate", 'process == "airportd" AND (eventMessage CONTAINS "LQM:" OR eventMessage CONTAINS "DatapathStall")'],
                     12, 400000)
    metrics = []
    stalls = []
    for line in result.get("output", "").splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        message = row.get("eventMessage", "")
        stamp = row.get("timestamp")
        if "DatapathStall" in message:
            stalls.append(stamp)
        if "LQM:" in message:
            # Numeric radio metrics only; omit network names and BSSIDs.
            fields = dict(re.findall(r"\b(\w+)=(-?\d+(?:\.\d+)?)", message))
            metrics.append({"time": stamp, "metrics": fields})
    return {"ok": result["ok"], "window_s": 90, "stall_times": stalls,
            "radio_metrics": metrics[-20:], "error": result.get("error")}


def wifi_summary():
    result = command(["/usr/sbin/system_profiler", "SPAirPortDataType", "-json"], 20, 2000000)
    if not result["ok"]:
        return result
    # Do not retain nearby network names, MAC addresses or the full scan.
    try:
        data = json.loads(result["output"])
    except (ValueError, KeyError):
        return {"ok": False, "error": "Wi-Fi report unavailable or truncated"}
    networks = []

    def walk(value):
        if isinstance(value, dict):
            for key, child in value.items():
                if "current_network" in key and isinstance(child, dict):
                    networks.append({k: v for k, v in child.items()
                                     if any(word in k.lower() for word in
                                            ("channel", "signal", "noise", "rate", "phymode", "mcs", "security"))})
                elif isinstance(child, (dict, list)):
                    walk(child)
        elif isinstance(value, list):
            for child in value:
                walk(child)
    walk(data)
    return {"ok": True, "current_networks": networks}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--interval", type=float, default=15, help="seconds between sample starts (minimum 10)")
    parser.add_argument("--duration-hours", type=float, default=24)
    parser.add_argument("--once", action="store_true", help="collect one sample and snapshot, then exit")
    parser.add_argument("--ap-ip", help="access point management IPv4 address; enables separate AP probes")
    parser.add_argument("--log-dir", type=Path, default=Path.home() / "Library/Logs/NetwatchDiagnostics")
    args = parser.parse_args()
    if args.interval < 10 or args.duration_hours <= 0:
        parser.error("interval must be >=10 and duration-hours must be positive")
    if args.ap_ip:
        try:
            ipaddress.IPv4Address(args.ap_ip)
        except ValueError:
            parser.error("ap-ip must be an IPv4 address")
    args.log_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(args.log_dir, 0o700)
    lock = open(args.log_dir / "monitor.pid", "a+")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("A monitor is already running in this log directory.", file=sys.stderr)
        return 1
    lock.seek(0)
    lock.truncate()
    lock.write(str(os.getpid()) + "\n")
    lock.flush()
    logger = logging.getLogger("connection-monitor")
    logger.setLevel(logging.INFO)
    logger.propagate = False
    handler = logging.handlers.RotatingFileHandler(args.log_dir / "connection.jsonl",
                                                  maxBytes=5 * 1024 * 1024, backupCount=4)
    handler.setFormatter(logging.Formatter("%(message)s"))
    logger.addHandler(handler)

    def emit(kind, **data):
        logger.info(json.dumps({"time": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                                "event": kind, **data}, ensure_ascii=True))

    stop = threading.Event()
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, lambda *_: stop.set())
    print(f"Monitor PID: {os.getpid()}\nLogs: {args.log_dir}\nRead-only; stops after {args.duration_hours:g} hours or on Ctrl-C/SIGTERM.", flush=True)
    emit("start", pid=os.getpid(), interval_s=args.interval, duration_hours=args.duration_hours,
         ap_ip=args.ap_ip, system=command(["/usr/bin/sw_vers"]))
    deadline = time.monotonic() + args.duration_hours * 3600
    last_snapshot = float("-inf")
    previous = None
    last_start = None
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
            while not stop.is_set() and time.monotonic() < deadline:
                started = time.monotonic()
                routing = route()
                probes = {
                    "router_ping": pool.submit(ping, routing.get("gateway")),
                    "router_tcp": pool.submit(local_tcp, routing.get("gateway")),
                    "internet_ping": pool.submit(ping, "1.1.1.1"),
                    "system_dns": pool.submit(command, ["/usr/bin/dscacheutil", "-q", "host", "-a", "name", "www.apple.com"], 5),
                    "https_normal": pool.submit(https),
                    "https_ipv6": pool.submit(https, False, True),
                    "https_without_dns": pool.submit(https, True),
                }
                if args.ap_ip:
                    probes["ap_ping"] = pool.submit(ping, args.ap_ip)
                    probes["ap_tcp"] = pool.submit(local_tcp, args.ap_ip)
                results = {name: future.result() for name, future in probes.items()}
                dns = results["system_dns"]
                dns["ok"] = dns["ok"] and "ip_address:" in dns.get("output", "")
                failed = [name for name, result in results.items() if not result["ok"]]
                slow = [name for name, result in results.items()
                        if name.startswith("https") and result.get("total_s", 0) > 2]
                # ICMP loss alone is not proof that web traffic is broken.
                state = (tuple(failed), tuple(slow), routing.get("gateway"), routing.get("interface"))
                emit("sample", route=routing, probes=results, failed=failed, slow=slow,
                     gap_s=round(started - last_start, 1) if last_start is not None else None)
                last_start = started
                changed = state != previous
                if changed:
                    emit("state_change", failed=failed, slow=slow, interface=routing.get("interface"))
                # Limit expensive snapshots even if ping results fluctuate frequently.
                if started - last_snapshot >= 300 or (changed and started - last_snapshot >= 60):
                    snapshots = {
                        "dns": ["/usr/sbin/scutil", "--dns"],
                        "vpn": ["/usr/sbin/scutil", "--nc", "list"],
                        "proxy": ["/usr/sbin/scutil", "--proxy"],
                        "reachability": ["/usr/sbin/scutil", "--nwi"],
                        "extensions": ["/usr/bin/systemextensionsctl", "list"],
                        "arp": ["/usr/sbin/arp", "-an"],
                    }
                    active_interface = routing.get("interface")
                    if active_interface:
                        snapshots["dhcp"] = ["/usr/sbin/ipconfig", "getpacket", active_interface]
                        snapshots["interface_counters"] = ["/usr/sbin/netstat", "-ibn", "-I", active_interface]
                    warp = shutil.which("warp-cli")
                    if warp:
                        snapshots["warp"] = [warp, "status"]
                    futures = {k: pool.submit(command, v, 8) for k, v in snapshots.items()}
                    wifi = pool.submit(wifi_summary)
                    radio = pool.submit(wifi_events)
                    backend = pool.submit(tailscale_backend)
                    emit("snapshot", **{k: f.result() for k, f in futures.items()}, wifi=wifi.result(),
                         wifi_events=radio.result(), tailscale_backend=backend.result())
                    last_snapshot = started
                previous = state
                if args.once:
                    break
                stop.wait(max(0, args.interval - (time.monotonic() - started)))
    finally:
        emit("stop")
        # Keep the lock inode in place so concurrent launches cannot bypass it.
        lock.seek(0)
        lock.truncate()
        lock.close()
        handler.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
