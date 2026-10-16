#!/usr/bin/env python3
"""Tmux FALSO do harness rev5 (so para testes; nunca executa o tmux real).

Uso (gerado pelo harness num wrapper sh): python3 -I fake_tmux.py DIR_CENARIO ARGV_DO_TMUX...

- Le DIR/cenario.json a cada chamada (o teste grava por rename atomico).
- Registra TODA chamada em DIR/chamadas.jsonl (uma linha JSON por registro: ev, pid, t, argv, rc).
- Lista fechada de subcomandos (spec rev5 secao 4 e 4A); qualquer outro e 'violacao' e sai 2:
    [-S SOCKET] list-panes -a -F FORMATO
    [-S SOCKET] capture-pane -p -e -J -t %N
    [-S SOCKET] -C attach -f ignore-size -t $N      (processo persistente; stdin so e contado)
- Trava de concorrencia (flock) entre list-panes/capture-pane: contencao vira 'sobreposicao'.
- Eventos de saida: DIR/eventos-N.txt (N = sessao sem '$'); linhas novas depois do inicio do
  processo de anexo saem no stdout. Linha '__EOF__' encerra; '__LONG__' imprime linha de 1,1 MiB.
"""
import fcntl
import json
import os
import re
import sys
import threading
import time

DIR = sys.argv[1]
ARGV = sys.argv[2:]
CEN = os.path.join(DIR, "cenario.json")
LOG = os.path.join(DIR, "chamadas.jsonl")
TRAVA = os.path.join(DIR, "trava.lock")
TOK = re.compile(r"#\{([^}]*)\}")
ALVO_PANE = re.compile(r"%[0-9]+\Z")
ALVO_SESSAO = re.compile(r"\$[0-9]+\Z")


def registra(**rec):
    rec.setdefault("pid", os.getpid())
    rec.setdefault("t", time.monotonic_ns())
    linha = (json.dumps(rec, ensure_ascii=False) + "\n").encode("utf-8")
    fd = os.open(LOG, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, linha)
    finally:
        os.close(fd)


def cenario():
    with open(CEN, "rb") as f:
        return json.loads(f.read().decode("utf-8"))


def stderr(msg):
    sys.stderr.write(msg + "\n")
    sys.stderr.flush()


def sentinela(c):
    s = c.get("stderr")
    if s:
        stderr(s)


def expande(fmt, painel):
    # Variavel desconhecida vira campo vazio (comportamento do tmux com -F).
    return TOK.sub(lambda m: str(painel.get(m.group(1), "")), fmt)


def list_panes(c, fmt):
    L = c.get("listar", {})
    if L.get("delay_ms"):
        time.sleep(L["delay_ms"] / 1000.0)
    sentinela(c)
    modo = L.get("modo", "ok")
    out = sys.stdout.buffer
    if modo == "sem_servidor":
        stderr("no server running on /tmp/tmux-0/default")
        return 1
    if modo == "falha":
        stderr("fake: falha na listagem")
        return 1
    if modo == "arquivo":
        with open(L["arquivo"], "rb") as f:
            out.write(f.read())
        out.flush()
        return 0
    if modo == "texto":
        out.write(L["texto"].encode("utf-8"))
        out.flush()
        return 0
    for p in c.get("paineis", []):
        out.write((expande(fmt, p) + "\n").encode("utf-8"))
    out.flush()
    return 0


def capture(c, alvo):
    sentinela(c)
    if not any(p.get("pane_id") == alvo for p in c.get("paineis", [])):
        stderr("can't find pane: " + alvo)
        return 1
    cap = c.get("capturas", {}).get(alvo, {})
    if cap.get("modo") == "falha":
        stderr("fake: captura falhou")
        return 1
    data = b""
    if cap.get("arquivo"):
        with open(cap["arquivo"], "rb") as f:
            data = f.read()
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()
    return 0


def attach(c, alvo):
    sentinela(c)
    if c.get("anexo_falha"):
        stderr("fake: anexo recusado")
        return 1
    feed = os.path.join(DIR, "eventos-%s.txt" % alvo[1:])
    fim_stdin = {"eof": False}

    def ler_stdin():
        # Conta os bytes que o daemon escreveu no stdin (spec R20: deve ser zero). Nunca interpreta.
        while True:
            try:
                d = os.read(0, 65536)
            except OSError:
                d = b""
            if not d:
                registra(ev="stdin_eof")
                fim_stdin["eof"] = True
                return
            registra(ev="stdin", n=len(d))

    threading.Thread(target=ler_stdin, daemon=True).start()
    off = os.path.getsize(feed) if os.path.exists(feed) else 0
    buf = b""
    while True:
        if fim_stdin["eof"]:
            return 0
        try:
            with open(feed, "rb") as f:
                f.seek(off)
                novo = f.read()
                off += len(novo)
        except FileNotFoundError:
            novo = b""
        buf += novo
        while b"\n" in buf:
            linha, buf = buf.split(b"\n", 1)
            if linha == b"__EOF__":
                return 0
            if linha == b"__LONG__":
                sys.stdout.buffer.write(b"%output %1 " + b"x" * 1100000 + b"\n")
                sys.stdout.buffer.flush()
                return 0
            sys.stdout.buffer.write(linha + b"\n")
            sys.stdout.buffer.flush()
            registra(ev="emitida", tam=len(linha))
            if linha.startswith(b"%exit"):
                return 0
        time.sleep(0.02)


def com_trava(fn):
    fd = os.open(TRAVA, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            registra(ev="sobreposicao", argv=ARGV)
            fcntl.flock(fd, fcntl.LOCK_EX)
        return fn()
    finally:
        os.close(fd)


def main():
    registra(ev="chamada", argv=ARGV)
    a = list(ARGV)
    if len(a) >= 2 and a[0] == "-S":
        a = a[2:]
    rc = 2
    try:
        if len(a) == 4 and a[0] == "list-panes" and a[1] == "-a" and a[2] == "-F":
            fmt = a[3]
            rc = com_trava(lambda: list_panes(cenario(), fmt))
        elif (
            len(a) == 6
            and a[0] == "capture-pane"
            and a[1:5] == ["-p", "-e", "-J", "-t"]
            and ALVO_PANE.match(a[5])
        ):
            alvo = a[5]
            rc = com_trava(lambda: capture(cenario(), alvo))
        elif (
            len(a) == 6
            and a[0:2] == ["-C", "attach"]
            and a[2:5] == ["-f", "ignore-size", "-t"]
            and ALVO_SESSAO.match(a[5])
        ):
            rc = attach(cenario(), a[5])
        else:
            registra(ev="violacao", argv=ARGV)
            stderr("fake tmux: subcomando proibido ou fora da lista")
            rc = 2
    except BrokenPipeError:
        rc = 0
    finally:
        registra(ev="fim", argv=ARGV, rc=rc)
    sys.stdout.flush()
    sys.exit(rc)


if __name__ == "__main__":
    main()
