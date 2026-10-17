"""Portao de mutacao do memd v0. Uso: mutacao.py DIR_TRABALHO DIR_TARGET (MUT_ONLY=S1,S2 filtra).
MORTO = algum teste falhou (bom). SOBREVIVEU = lacuna de teste ou mutante equivalente (justificar)."""
import os, shutil, subprocess, sys, time
SRC=os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))); W=sys.argv[1]; TD=sys.argv[2]
if os.path.exists(W): shutil.rmtree(W)
os.makedirs(W+"/crates")
for f in ("Cargo.toml","Cargo.lock","rust-toolchain.toml"):
    if os.path.exists(f"{SRC}/{f}"): shutil.copy(f"{SRC}/{f}",W)
for c in ("chatd","agentd","attest","blocklog","memd","grantd"): shutil.copytree(f"{SRC}/crates/{c}",f"{W}/crates/{c}",ignore=shutil.ignore_patterns("target"))
S="crates/memd/src/service.rs"; G="crates/memd/src/grants.rs"
M=[
("G1",G,".is_none_or(|expires| today <= expires)",".is_none_or(|expires| today < expires)","token vale ate o dia de expires (borda)"),
("G2",G,"scope == base || scope.strip_prefix(base)","scope.strip_prefix(base)","a/** deixa de casar a"),
("G3",G,"s.starts_with('/'))","true)","a/** casa 'ab' (prefixo sem fronteira)"),
("G4",G,"self.permits_type(kind) && self.read_scopes","self.read_scopes","leitura ignora types"),
("G5",G,"self.ops.iter().any(|item| item == op)\n            && self.permits_type","self.permits_type","escrita ignora ops"),
("G6",G,"&& self.write_scopes.iter()","&& self.read_scopes.iter()","escrita usa read_scopes"),
("G7",G,"&& self.permits_type(kind)\n            && self.write_scopes","\n            && self.write_scopes","escrita ignora types"),
("G8",G,"&& token.starts_with(\"mt_\")","&& true","prefixo do token ignorado"),
("S1",S,"if open >= self.cfg.max_open_proposals ||","if open > self.cfg.max_open_proposals ||","fronteira da cota de propostas abertas"),
("S2",S,"self.state.record_count() >= self.cfg.max_records","self.state.record_count() > self.cfg.max_records","fronteira de max_records"),
("S3",S,"if p.expected_rev != Some(active_rev) {","if false {","expected_rev ignorado"),
("S4",S,"if p.expected_seq != record.proposed_seq {","if false {","expected_seq ignorado"),
("S5",S,"if now.signed_duration_since(at.with_timezone(&Utc)) >= ttl {","if now.signed_duration_since(at.with_timezone(&Utc)) > ttl {","expire no limite exato do ttl"),
("S6",S,"due.truncate(self.cfg.view_overdue_max);","","view_overdue_max ignorado"),
("S7",S,"text.contains(\"-----BEGIN\") && text.contains(\"PRIVATE KEY-----\")","false","chave privada nao detectada"),
("S8",S,"token_after(text, \"ghp_\", 36,","token_after(text, \"ghp_\", 37,","ghp_ com 36 nao detectado"),
("S9",S,"        \"AKIA\",\n        16,","        \"AKIA\",\n        17,","AKIA com 16 nao detectado"),
("S10",S,"if record.status != \"proposed\" {\n            return Err(err(\"conflict\"","if false {\n            return Err(err(\"conflict\"","promote de nao proposed aceito"),
("S11",S,"if record.status != expected {\n            return Err(err(\"conflict\", \"estado da versao incompativel\"));\n        }\n        let event = json!","if false {\n            return Err(err(\"conflict\", \"estado da versao incompativel\"));\n        }\n        let event = json!","reject/retire em estado errado aceito"),
("S12",S,"&& !record.supersedes.contains(&other.id)","","conflito ignora supersedes"),
("S13",S,"other.id != record.id\n                    && other.kind","other.kind","conflito conta o proprio id"),
("S14",S,".is_some_and(|d| d < self.effective_now.date_naive())",".is_some_and(|d| d <= self.effective_now.date_naive())","unconfirmed no proprio dia de review_by"),
("S15",S,"if !grant.can_write(\"promote\", &record.kind, &record.scope) {\n            return Err(err(\"forbidden\", \"operacao proibida\"));\n        }\n        if record.status != \"proposed\"","if false {\n            return Err(err(\"forbidden\", \"operacao proibida\"));\n        }\n        if record.status != \"proposed\"","promote sem checar escopo de escrita"),
("S16",S,"if !grant.can_read(&record.kind, &record.scope) {\n            return Err(not_found());\n        }\n        if !grant.can_write(\"promote\", &record.kind, &record.scope) {\n            return Err(err(\"forbidden\", \"operacao proibida\"));\n        }\n        if record.status != \"proposed\"","if false {\n            return Err(not_found());\n        }\n        if !grant.can_write(\"promote\", &record.kind, &record.scope) {\n            return Err(err(\"forbidden\", \"operacao proibida\"));\n        }\n        if record.status != \"proposed\"","promote sem checar leitura (A4)"),
("S17",S,"if !grant.can_write(\"promote\", &target.kind, &target.scope) {\n                return Err(not_found());","if false {\n                return Err(not_found());","supersedes sem escrita no alvo"),
("S18",S,"CommitError::LogFull => err(\"quota\", \"log cheio\")","CommitError::LogFull => err(\"broken\", \"log cheio\")","LogFull com codigo errado"),
]
env=dict(os.environ,CARGO_TARGET_DIR=TD)
ONLY=os.environ.get("MUT_ONLY","").split(",") if os.environ.get("MUT_ONLY") else None
for mid,f,a,b,desc in M:
    if ONLY and mid not in ONLY: continue
    p=f"{W}/{f}"; orig=open(p).read()
    if orig.count(a)!=1: print(mid,"TRECHO-NAO-UNICO",orig.count(a),flush=True); continue
    open(p,"w").write(orig.replace(a,b,1)); t=time.time()
    r=subprocess.run(["cargo","test","--offline","-p","memd","--no-fail-fast"],cwd=W,env=env,capture_output=True,text=True)
    out=r.stdout+r.stderr
    st="NAO-COMPILA" if ("could not compile" in out and "test result" not in out) else ("SOBREVIVEU" if r.returncode==0 else "MORTO")
    print(mid,st,desc,"%ds"%(time.time()-t),flush=True)
    open(p,"w").write(orig)
print("FIM")
