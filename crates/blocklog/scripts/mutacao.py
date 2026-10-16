"""Portao de mutacao do blocklog v0. Uso: mutacao.py DIR_TRABALHO DIR_TARGET. Um mutante por vez; roda os testes de integracao
independentes. MORTO = algum teste falhou (bom). SOBREVIVEU = lacuna de teste ou mutante equivalente (justificar)."""
import os, shutil, subprocess, sys, time
SRC=os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))); W=sys.argv[1]; TD=sys.argv[2]
if os.path.exists(W): shutil.rmtree(W)
os.makedirs(W+"/crates")
for f in ("Cargo.toml","Cargo.lock","rust-toolchain.toml"): shutil.copy(f"{SRC}/{f}",W)
for c in ("chatd","agentd","attest","blocklog"): shutil.copytree(f"{SRC}/crates/{c}",f"{W}/crates/{c}",ignore=shutil.ignore_patterns("target"))
L="crates/blocklog/src/log.rs"; F="crates/blocklog/src/fs_storage.rs"
M=[
("L1",L,"if line.len() > self.opts.line_max {","if line.len() >= self.opts.line_max {","linha exatamente line_max recusada"),
("L2",L,"let ts = self.last_ts.map_or(now, |last| now.max(last));","let ts = now;","ts deixa de ser nao regressivo"),
("L3",L,".is_none_or(|expiry| expiry > ts)\n        });\n        if let Some(req)",".is_none_or(|expiry| expiry >= ts)\n        });\n        if let Some(req)","fronteira exata da retencao em commit (ambiguidade A1 do Haiku)"),
("L4",L,"Some(entry) if entry.h != req.h => Err(CommitError::RequestConflict),","Some(entry) if false => Err(CommitError::RequestConflict),","h diferente deixa de ser conflito"),
("L5",L,"if complete_len != bytes.len() {\n        file.set_len(complete_len as u64)?;","if false {\n        file.set_len(complete_len as u64)?;","cauda cortada nao e truncada"),
("L6",L,"file.set_len(complete_len as u64)?;\n        file.sync_all()?;","file.set_len(complete_len as u64)?;","sem sync_all apos truncar"),
("L7",L,"if previous_seq.checked_add(1) != Some(stored.seq) {","if stored.seq <= previous_seq {","lacuna de seq aceita"),
("L8",L,"if previous_ts.is_some_and(|previous| parsed_ts < previous) {","if false {","ts regressivo aceito na abertura"),
("L9",L,"if stored.v != 1 {","if false {","versao diferente de 1 aceita"),
("L10",L,"if req.is_some() && res.is_none() {\n        return Err(corrupt(line_number, \"req sem res\"));","if false {\n        return Err(corrupt(line_number, \"req sem res\"));","req sem res aceito na abertura"),
("L11",L,"self.broken = true;\n        Err(CommitError::Io(error))","Err(CommitError::Io(error))","falha de escrita nao marca broken"),
("L12",L,"|expiry| *ts < expiry)","|_| false)","duplicata viva de req nao e corrupcao"),
("L13",L,".is_none_or(|expiry| expiry > opened_at)",".is_none_or(|_| true)","req vencida volta ao indice apos reabrir"),
("L14",L,"|| req.agent.len() > 32","|| req.agent.len() > 33","agent com 33 caracteres aceito"),
("L15",L,"if req.h.len() != 64","if req.h.len() > 64","h com menos de 64 aceito"),
("L16",L,"byte.is_ascii_lowercase() || byte.is_ascii_digit() || b\"_-\".contains(&byte)","byte.is_ascii_alphabetic() || byte.is_ascii_digit() || b\"_-\".contains(&byte)","agent com maiuscula aceito"),
("L17",L,"if tx.len() > opts.tx_max {\n        return Err(CommitError::TxTooLarge);","if tx.len() > opts.tx_max + 1 {\n        return Err(CommitError::TxTooLarge);","tx com tx_max+1 eventos aceito"),
("L18",L,".saturating_add(1) > self.opts.log_max_bytes {",".saturating_add(1) >= self.opts.log_max_bytes {","fronteira exata de log_max_bytes"),
("L19",L,"if let Err(error) = self.file.sync_data() {\n            return self.fail_write(error);\n        }","","sync_data ignorado"),
("L20",L,"if line.len() > opts.line_max {\n        return Err(corrupt(line_number, \"linha acima de line_max\"));","if false {\n        return Err(corrupt(line_number, \"linha acima de line_max\"));","linha maior que line_max aceita na abertura"),
("F1",F,"metadata.mode() & 0o077 != 0 || metadata.uid() != uid","metadata.mode() & 0o007 != 0 || metadata.uid() != uid","grupo com acesso ao arquivo tolerado (as duas camadas relaxadas)"),
("F2",F,"let flags = libc::O_CLOEXEC | libc::O_NOFOLLOW;","let flags = libc::O_CLOEXEC;","symlink seguido (sem O_NOFOLLOW e sem a checagem lstat)"),
("F4",F,"File::open(&self.dir)?.sync_all()?;","","sem fsync do diretorio apos criar (nao observavel pela API)"),
]
EXTRA={
 "F1":[(F,"|| metadata.mode() & 0o777 != 0o600","|| metadata.mode() & 0o707 != 0o600")],
 "F2":[(F,"let metadata = match fs::symlink_metadata(path) {","if true { return Ok(()); }\n        let metadata = match fs::symlink_metadata(path) {")],
}
env=dict(os.environ,CARGO_TARGET_DIR=TD)
ONLY=os.environ.get("MUT_ONLY","").split(",") if os.environ.get("MUT_ONLY") else None
for mid,f,a,b,desc in M:
    if ONLY and mid not in ONLY: continue
    edits=[(f,a,b)]+EXTRA.get(mid,[])
    origs={}; bad=False
    for (ff,aa,bb) in edits:
        pp=f"{W}/{ff}"; origs[pp]=origs.get(pp) or open(pp).read()
        if origs[pp].count(aa)!=1: print(mid,"TRECHO-NAO-UNICO",origs[pp].count(aa),flush=True); bad=True; break
    if bad: continue
    for (ff,aa,bb) in edits:
        pp=f"{W}/{ff}"; cur=open(pp).read(); open(pp,"w").write(cur.replace(aa,bb,1))
    t=time.time()
    r=subprocess.run(["cargo","test","--offline","-p","blocklog","--no-fail-fast"],cwd=W,env=env,capture_output=True,text=True)
    out=r.stdout+r.stderr
    st="NAO-COMPILA" if ("could not compile" in out or "error[" in out) else ("SOBREVIVEU" if r.returncode==0 else "MORTO")
    print(mid,st,desc,"%ds"%(time.time()-t),flush=True)
    for pp,o in origs.items(): open(pp,"w").write(o)
print("FIM")
