"""Portao de mutacao da v1.10 do chatd (campainha tmux sem digitar; rearme duravel).
Uso: mutacao_v110.py DIR_TRABALHO DIR_TARGET. Copia o workspace e aplica UM mutante por vez;
roda so v110_campainha e bind_v13. MORTO = algum teste falhou (bom)."""
import os, shutil, subprocess, sys, time
SRC=os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))); W=sys.argv[1]; TD=sys.argv[2]
if os.path.exists(W): shutil.rmtree(W)
os.makedirs(W+"/crates")
for f in ("Cargo.toml","Cargo.lock","rust-toolchain.toml"): shutil.copy(f"{SRC}/{f}",W)
for c in ("chatd","agentd","attest"): shutil.copytree(f"{SRC}/crates/{c}",f"{W}/crates/{c}",ignore=shutil.ignore_patterns("target"))
M=[
("B1","crates/chatd/src/bell.rs","if target.mux == Mux::Tmux && !shared.cfg.bell_tmux_typing {","if false {","guarda de ring removida (verify roda display-message do tmux)"),
("B2","crates/chatd/src/bell.rs","        Mux::Tmux if !shared.cfg.bell_tmux_typing => Err(\"tmux_typing_disabled\"),\n","","guarda de send_prompt removida (defesa em profundidade)"),
("C1","crates/chatd/src/config.rs","raw.bell_tmux_typing.unwrap_or(false)","raw.bell_tmux_typing.unwrap_or(true)","padrao da chave vira true"),
("S1","crates/chatd/src/store.rs","replay_bell_instant(&ts, Utc::now(), Instant::now())","Instant::now()","replay ignora a idade (sempre espera o rearme inteiro)"),
("S2","crates/chatd/src/store.rs","                self.outstanding\n                    .insert(r, replay_bell_instant(&ts, Utc::now(), Instant::now()));\n","                let _ = r;\n","replay nao reconstroi outstanding"),
("S3","crates/chatd/src/store.rs","        .unwrap_or_default();\n    monotonic_now","        .unwrap_or(Duration::from_secs(100000));\n    monotonic_now","ts no futuro ou ilegivel vira idade enorme"),
("S4","crates/chatd/src/store.rs","monotonic_now.checked_sub(age).unwrap_or(monotonic_now)","monotonic_now.checked_sub(age).unwrap_or(monotonic_now - Duration::from_secs(0))","equivalente (controle): sem efeito observavel"),
]
env=dict(os.environ,CARGO_TARGET_DIR=TD)
for mid,f,a,b,desc in M:
    p=f"{W}/{f}"; orig=open(p).read()
    if orig.count(a)!=1: print(mid,"TRECHO-NAO-UNICO",orig.count(a),flush=True); continue
    open(p,"w").write(orig.replace(a,b,1)); t=time.time()
    r=subprocess.run(["cargo","test","--offline","-p","chatd","--test","v110_campainha","--test","bind_v13"],cwd=W,env=env,capture_output=True,text=True)
    out=r.stdout+r.stderr
    st="NAO-COMPILA" if ("could not compile" in out or "error[" in out) else ("SOBREVIVEU" if r.returncode==0 else "MORTO")
    print(mid,st,desc,"%ds"%(time.time()-t),flush=True)
    open(p,"w").write(orig)
print("FIM")
