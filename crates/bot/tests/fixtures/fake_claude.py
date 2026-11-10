#!/usr/bin/env python3
"""A stand-in for `claude -p --output-format stream-json`. It never talks to any service. It records how it was started,
then plays one scenario named on the first line of the prompt, asking the permission-prompt MCP server like the real CLI."""
import sys, json, os, subprocess, time

args = sys.argv[1:]
prompt = sys.stdin.read()
first = prompt.strip().splitlines()[0] if prompt.strip() else ""
out = os.environ.get("FAKE_CLAUDE_OUT")
if out:
    with open(out, "a") as f:
        f.write(json.dumps({"args": args, "prompt": prompt, "cwd": os.getcwd(), "env": sorted(os.environ.keys())}) + "\n")
pidfile = os.environ.get("FAKE_CLAUDE_PID")
if pidfile:
    open(pidfile, "w").write(str(os.getpid()))

def emit(o):
    print(json.dumps(o), flush=True)

def flag(name):
    return args[args.index(name) + 1] if name in args else None

session = flag("--resume") or "sess-1"
emit({"type": "system", "subtype": "init", "session_id": session})

def result(text, err=False):
    emit({"type": "result", "subtype": "error_during_execution" if err else "success", "is_error": err, "result": text, "session_id": session})
    sys.exit(0)

class Mcp:
    def __init__(self):
        cfg = json.load(open(flag("--mcp-config")))["mcpServers"]["approver"]
        env = dict(os.environ); env.update(cfg.get("env", {}))
        self.p = subprocess.Popen([cfg["command"]] + cfg["args"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, text=True)
        self.n = 0
        self.rpc("initialize", {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "fake", "version": "0"}})
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"); self.p.stdin.flush()
    def rpc(self, method, params):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}) + "\n"); self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())["result"]
    def ask(self, tool, inp, tid):
        r = self.rpc("tools/call", {"name": "ask", "arguments": {"tool_name": tool, "input": inp, "tool_use_id": tid}})
        return json.loads(r["content"][0]["text"])

def tool_step(mcp, tool, inp, tid):
    emit({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": tid, "name": tool, "input": inp}]}})
    d = mcp.ask(tool, inp, tid)
    if d["behavior"] == "allow":
        used = d["updatedInput"]
        emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid, "content": "ok", "is_error": False}]}})
        return True, used, None
    emit({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid, "content": d["message"], "is_error": True}]}})
    return False, None, d["message"]

parts = first.split(" ", 2)
scenario = parts[1] if len(parts) > 1 else "answer"
rest = parts[2] if len(parts) > 2 else ""
if scenario == "sleep":
    time.sleep(120)
elif scenario in ("bash", "read", "write"):
    mcp = Mcp()
    if scenario == "bash":
        ok, used, msg = tool_step(mcp, "Bash", {"command": rest, "description": "test"}, "tu1")
        result("ran: " + used["command"] if ok else "denied: " + msg)
    elif scenario == "read":
        ok, used, msg = tool_step(mcp, "Read", {"file_path": os.path.expanduser(rest)}, "tu1")
        result("read it" if ok else "denied: " + msg)
    else:
        ok, used, msg = tool_step(mcp, "Write", {"file_path": os.path.expanduser(rest), "content": "hello\nworld\n"}, "tu1")
        result("wrote it" if ok else "denied: " + msg)
elif scenario == "badresume":
    result("No conversation found with session ID", True)
else:
    emit({"type": "assistant", "message": {"content": [{"type": "text", "text": "thinking"}]}})
    result("all done")
