import sys, json, time, subprocess
sys.path.insert(0, ".")
from ws_codex import WS
T = "01a145f9-b71e-7552-a7f0-554ae87ffbc8"
w = WS()
w.call(1, "initialize", {"clientInfo": {"name": "sextante-leitura", "title": None, "version": "0"}, "capabilities": {"experimentalApi": True}}); w.send({"method": "initialized"})
n = [10]
def nid(): n[0] += 1; return n[0]
def estado():
    m = w.call(nid(), "thread/read", {"threadId": T, "includeTurns": False}); return ((m or {}).get("result", {}).get("thread", {}) or {}).get("status")
def fila():
    m = w.call(nid(), "thread/queue/list", {"threadId": T})
    if not m or "error" in m: return "ERRO " + json.dumps(m)[:120]
    return [((x.get("input") or [{}])[0].get("text", ""))[:30] for x in m["result"].get("data", [])]
def q(msg):
    return subprocess.run(["codex","queue","--thread",T,"--message",msg],capture_output=True,text=True,timeout=30).returncode
def log(*a): print("t+%5.1fs" % (time.time() - t0), *a, flush=True)
t0 = time.time()
log("inicio: estado", estado(), "fila", fila())
log("A rc=", q("Rode no shell exatamente: curl -sI https://example.com | head -1 . Se for negado ou falhar, responda somente: negado. Se funcionar, responda somente: A-ok"))
ult = None; achou = False; c_enviada = False
while time.time() - t0 < 240:
    s = estado(); f = fila(); cur = (json.dumps(s), json.dumps(f))
    if cur != ult: log("estado=", json.dumps(s), "fila=", f); ult = cur
    flags = (s or {}).get("activeFlags", []) if isinstance(s, dict) else []
    if flags and not c_enviada:
        achou = True
        log("APROVACAO ABERTA (flags=%s). Enfileirando C com a aprovacao pendente..." % flags)
        log("C rc=", q("Responda somente: C-ok")); c_enviada = True
    if isinstance(s, dict) and s.get("type") == "idle" and time.time() - t0 > 6 and (c_enviada is False or f == []):
        # idle e fila vazia depois de ter passado por tudo
        if c_enviada or not achou: break
    time.sleep(1)
m = w.call(900, "thread/read", {"threadId": T, "includeTurns": True})
th = (m or {}).get("result", {}).get("thread", {})
log("--- ultimos turnos:")
for t in th.get("turns", [])[-3:]:
    outs = []
    for it in t.get("items", []):
        tipo = it.get("type")
        if tipo == "userMessage": outs.append("USER: " + (it.get("content") or [{}])[0].get("text","")[:45])
        elif tipo == "agentMessage": outs.append("AGENT: " + (it.get("text") or "")[:40])
        elif tipo == "commandExecution": outs.append("CMD(%s): %s" % (it.get("status"), str(it.get("command"))[:40]))
    log(t.get("status"), outs)
w.close(); log("FIM")
