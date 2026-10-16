"""Portao de mutacao da v1.11 do chatd (canal codex-queue).
Uso: mutacao_v111.py DIR_TRABALHO DIR_TARGET. Copia o workspace e aplica UM mutante por vez;
roda v111_codex_queue (sem os lentos q19/q20, exceto nos mutantes que os miram). MORTO = algum teste falhou (bom)."""
import os, shutil, subprocess, sys, time
SRC=os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))); W=sys.argv[1]; TD=sys.argv[2]
if os.path.exists(W): shutil.rmtree(W)
os.makedirs(W+"/crates")
for f in ("Cargo.toml","Cargo.lock","rust-toolchain.toml"): shutil.copy(f"{SRC}/{f}",W)
for c in ("chatd","agentd","attest"): shutil.copytree(f"{SRC}/crates/{c}",f"{W}/crates/{c}",ignore=shutil.ignore_patterns("target"))
Q="crates/chatd/src/codex_queue.rs"; B="crates/chatd/src/bell.rs"; C="crates/chatd/src/config.rs"
# (id, arquivo, trecho, troca, descricao, filtro: None = tudo menos q19/q20; "q19"/"q20" = so esse)
M=[
("M1",Q,'Ok(out) if out.starts_with("Queued message ") => Ok(()),','Ok(_) => Ok(()),',"sucesso sem checar a saida do queue",None),
("M2",Q,"    codex_version(&config.codex_bin, &config.codex_versions)?;\n","","checagem de versao removida",None),
("M3",Q,"metadata.mode() & 0o022 != 0","metadata.mode() & 0o002 != 0","escrita de grupo aceita",None),
("M4",Q,"metadata.mode() & 0o022 != 0","metadata.mode() & 0o020 != 0","escrita de outros aceita",None),
("M5",Q,"|| metadata.uid() != daemon_uid","|| false","dono do arquivo nao checado (esperado: SOBREVIVE, nao testavel sem root)",None),
("M6",Q,"if bytes.last() == Some(&b'\\n') {","while bytes.last() == Some(&b'\\n') || bytes.last() == Some(&b'\\r') {","aceita varias quebras e CRLF",None),
("M7",Q,"byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)","byte.is_ascii_hexdigit()","UUID maiusculo aceito",None),
("M8",Q,'"--thread",','"--session",',"opcao do argv trocada",None),
("M9",Q,"let message = prompt_text(pending);",'let message = String::from("x");',"texto enviado deixa de ser o fixo",None),
("M10",Q,"if !allowed_versions.iter().any(|allowed| allowed == &version) {","if false {","versao fora da lista aceita na 1a consulta",None),
("M11",Q,'"spawn_failed" | "timeout" => reason,','"spawn_failed" => reason,',"timeout do --version vira unsupported (provavel: SOBREVIVE, lacuna conhecida)",None),
("M12",Q,"let thread = read_thread_id(thread_file)?;\n    codex_version(","codex_version(","(nao aplica)",None),
("M13",B,"Duration::from_secs(60)","Duration::from_secs(0)","sem espera de 60 s apos falha","q19"),
("M14",B,'Some(if reason == "timeout" {','Some(if false {',"timeout nao espera bell_rearm_s","q20"),
("M15",C,"        && codex_versions.is_empty()","        && false","codex_versions deixa de ser obrigatoria",None),
("M16",C,"if Path::new(&path).is_absolute() =>","if true =>","thread_file relativo aceito",None),
("M17",C,'(Some("codex-queue"), None, Some(path))','(Some("codex-queue"), _, Some(path))',"pane aceito com codex-queue",None),
("M18",C,"Delivery::Bell { .. } | Delivery::Auto | Delivery::CodexQueue { .. }","Delivery::Bell { .. } | Delivery::Auto","codex-queue deixa de ser campainha (nunca devido)",None),
]
env=dict(os.environ,CARGO_TARGET_DIR=TD)
for mid,f,a,b,desc,filt in M:
    if mid=="M12": continue
    p=f"{W}/{f}"; orig=open(p).read()
    if orig.count(a)!=1: print(mid,"TRECHO-NAO-UNICO",orig.count(a),flush=True); continue
    open(p,"w").write(orig.replace(a,b,1)); t=time.time()
    cmd=["cargo","test","--offline","-p","chatd","--test","v111_codex_queue","--"]
    cmd+= [filt] if filt else ["--skip","q19","--skip","q20"]
    r=subprocess.run(cmd,cwd=W,env=env,capture_output=True,text=True)
    out=r.stdout+r.stderr
    st="NAO-COMPILA" if ("could not compile" in out or "error[" in out) else ("SOBREVIVEU" if r.returncode==0 else "MORTO")
    print(mid,st,desc,"%ds"%(time.time()-t),flush=True)
    open(p,"w").write(orig)
print("FIM")
