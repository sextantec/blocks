"""Portao de mutacao do agentd rev6 (estado estruturado + fusao).
Uso: mutacao_rev6.py DIR_TRABALHO DIR_TARGET. Copia o workspace e aplica UM mutante por vez;
roda so os alvos rev6_*. MORTO = algum teste falhou (bom). SOBREVIVEU = lacuna de teste."""
import os, shutil, subprocess, sys, time
SRC=os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))); W=sys.argv[1]; TD=sys.argv[2]
if os.path.exists(W): shutil.rmtree(W)
os.makedirs(W+"/crates")
for f in ("Cargo.toml","Cargo.lock","rust-toolchain.toml"): shutil.copy(f"{SRC}/{f}",W)
for c in ("chatd","agentd","attest"): shutil.copytree(f"{SRC}/crates/{c}",f"{W}/crates/{c}",ignore=shutil.ignore_patterns("target"))
D="crates/agentd/src/daemon.rs"; A="crates/agentd/src/app_server.rs"; C="crates/agentd/src/config.rs"
M=[
("F1",D,'        if state == "working"\n            && old_report','        if true\n            && old_report',"override tambem em blocked"),
("F2",D,'            && now.duration_since(since).as_secs() >= cfg.override_after_s\n','            && true\n',"override sem esperar a tela provar idle por override_after_s"),
("F3",D,'.is_some_and(|reported| now.duration_since(reported).as_secs() >= cfg.override_after_s);','.is_some_and(|_| true);',"override sem esperar a idade do relato"),
("F4",D,'if a.screen_state == "idle" && a.screen_rule.is_some() {','if a.screen_state == "idle" {',"override sem regra positiva de idle na tela"),
("F5",D,'a.terminal.is_none() || a.terminal_confirmations < 2','false',"idle estruturado sem confirmar o terminal"),
("F6",D,'_ if matches!(a.screen_state.as_str(), "working" | "blocked") => {','_ if false => {',"idle relatado vence tela em working/blocked"),
("F7",D,'if now.duration_since(since).as_millis() as u64 >= cfg.diverge_after_ms {','if true {',"diverged sem esperar diverge_after_ms"),
("F8",D,'.is_some_and(|t| now.duration_since(t).as_secs() >= cfg.hooks_silence_s)','.is_some_and(|_| true)',"hooks_silence_s ignorado"),
("F9",D,'a.evidence = if a.screen_state == state {','a.evidence = if true {',"evidence both sempre"),
("B1",D,'if state == "working" && a.screen_state == "blocked" && a.screen_rule.is_some() {','if state == "working" && false && a.screen_rule.is_some() {',"tela blocked nao rebaixa working relatado"),
("B2",D,'if state == "working" && a.screen_state == "blocked" && a.screen_rule.is_some() {','if a.screen_state == "blocked" && a.screen_rule.is_some() {',"tela blocked tambem rebaixa blocked relatado"),
("B3",D,'if state == "working" && a.screen_state == "blocked" && a.screen_rule.is_some() {','if state == "working" && a.screen_state == "blocked" {',"blocked da tela sem regra positiva (equivalente provavel)"),
("H1",D,'"UserPromptSubmit" => Some(("working", None)),','"UserPromptSubmit" => Some(("idle", None)),',"UserPromptSubmit vira idle"),
("H2",D,'.is_some_and(|t| t.0 == pane)','.is_some_and(|_| true)',"relato aceito de qualquer painel"),
("H3",D,'"Elicitation" => Some(("blocked", Some("user_input"))),','"Elicitation" => Some(("blocked", Some("permission"))),',"Elicitation com razao errada"),
("S1",A,'if flags.iter().any(|f| f == "waitingOnApproval") {','if false {',"waitingOnApproval ignorado"),
("S2",A,'.any(|f| !matches!(f.as_str(), Some("waitingOnApproval" | "waitingOnUserInput")))','.any(|_| false)',"activeFlag desconhecido aceito"),
("S3",A,'"notLoaded" => unknown(true),','"notLoaded" => unknown(false),',"notLoaded sem structured_lost"),
("C1",C,'(5..=300).contains(&l.override_after_s)','(0..=300).contains(&l.override_after_s)',"override_after_s aceita 0"),
("C2",C,'(1000..=60000).contains(&l.diverge_after_ms)','(0..=60000).contains(&l.diverge_after_ms)',"diverge_after_ms aceita 0"),
]
env=dict(os.environ,CARGO_TARGET_DIR=TD)
T=[x for t in ("rev6_codex","rev6_hooks","rev6_fusao","rev6_config_privacidade","rev6_lacunas") for x in ("--test",t)]
ONLY=os.environ.get("MUT_ONLY","").split(",") if os.environ.get("MUT_ONLY") else None
for mid,f,a,b,desc in M:
    if ONLY and mid not in ONLY: continue
    p=f"{W}/{f}"; orig=open(p).read()
    if orig.count(a)!=1: print(mid,"TRECHO-NAO-UNICO",orig.count(a),flush=True); continue
    open(p,"w").write(orig.replace(a,b,1)); t=time.time()
    SK=os.environ.get("MUT_SKIP"); X=["--","--skip",SK] if SK else []
    r=subprocess.run(["cargo","test","--offline","-p","agentd","--no-fail-fast"]+T+X,cwd=W,env=env,capture_output=True,text=True)
    out=r.stdout+r.stderr
    st="NAO-COMPILA" if ("could not compile" in out or "error[" in out) else ("SOBREVIVEU" if r.returncode==0 else "MORTO")
    print(mid,st,desc,"%ds"%(time.time()-t),flush=True)
    open(p,"w").write(orig)
print("FIM")
