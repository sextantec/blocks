"""Portao de mutacao do grantd v0. Uso: mutacao.py DIR_TRABALHO DIR_TARGET (MUT_ONLY=S1,S2 filtra).
MORTO = algum teste falhou (bom). SOBREVIVEU = lacuna de teste ou mutante equivalente (justificar)."""
import os, shutil, subprocess, sys, time
SRC=os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))); W=sys.argv[1]; TD=sys.argv[2]
if os.path.exists(W): shutil.rmtree(W)
os.makedirs(W+"/crates")
for f in ("Cargo.toml","Cargo.lock","rust-toolchain.toml"):
    if os.path.exists(f"{SRC}/{f}"): shutil.copy(f"{SRC}/{f}",W)
for c in ("chatd","agentd","attest","blocklog","memd","grantd"): shutil.copytree(f"{SRC}/crates/{c}",f"{W}/crates/{c}",ignore=shutil.ignore_patterns("target"))
L="crates/grantd/src/lib.rs"
M=[
("L1",L,"if (m == \"use\" || pf.role != \"owner\") && agent != pf.agent {","if false {","use/check de outro agente aceito"),
("L2",L,"if (m == \"use\" || pf.role != \"owner\") && agent != pf.agent {","if (pf.role != \"owner\") && agent != pf.agent {","owner usa em nome de terceiro"),
("L3",L,".is_some_and(|t| *now < t)\n        {\n            \"future\"",".is_some_and(|t| *now <= t)\n        {\n            \"future\"","not_before exclusivo"),
("L4",L,".is_some_and(|u| *now < u)\n        {\n            \"active\"",".is_some_and(|u| *now <= u)\n        {\n            \"active\"","until inclusivo"),
("L5",L,"if !parent.delegable {\n            return Err((\"exceeds_parent\", \"exceeds parent\"));","if false {\n            return Err((\"exceeds_parent\", \"exceeds parent\"));","delegar de pai nao delegable"),
("L6",L,"|| parent.grantee != pf.agent\n        {","\n        {","delegar concessao alheia"),
("L7",L,"if self.children(&parent.id) >= 3 {","if self.children(&parent.id) >= 4 {","max_children 4"),
("L8",L,"&& depth < 2\n            && (!delegable || depth < 1)","&& (!delegable || depth < 1)","sem limite de profundidade"),
("L9",L,"&& depth < 2\n            && (!delegable || depth < 1)","&& depth < 2","delegable do filho sem profundidade restante"),
("L10",L,"u.is_some_and(|x| pu.is_none_or(|p| x <= p))","u.is_some()","until do filho acima do pai"),
("L11",L,"&& constraints_leq(&parent.constraints, event.get(\"constraints\"))\n","\n","constraints do filho livres"),
("L12",L,"c.max_uses.is_some_and(|x| x <= p))","c.max_uses.is_some())","max_uses do filho acima do pai"),
("L13",L,"c.max_amount.is_some_and(|x| x <= p) && parent.unit == c.unit)","c.max_amount.is_some_and(|x| x <= p))","unit do filho diferente"),
("L14",L,"&& parent.window_s == c.window_s\n","\n","window_s do filho diferente"),
("L15",L,"x == v[0] && go(&p[1..], &v[1..])","x == v[0] || go(&p[1..], &v[1..])","glob: segmento literal frouxo"),
("L16",L,"\"*\" => !v.is_empty() && go(&p[1..], &v[1..]),","\"*\" => go(&p[1..], &v[1..]),","glob: * casa zero segmentos"),
("L17",L,"\"**\" => (0..=v.len()).any(|n| go(&p[1..], &v[n..])),","\"**\" => (1..=v.len()).any(|n| go(&p[1..], &v[n..])),","glob: ** exige um segmento"),
("L18",L,"if spent >= max || spent.saturating_add(q) > max {","if spent.saturating_add(q) > max {","max_amount: saldo exato com qty 0"),
("L19",L,"if spent >= max || spent.saturating_add(q) > max {","if spent >= max {","max_amount: qty acima do saldo"),
("L20",L,".is_some_and(|max| us.iter().filter(|u| u.ts >= start).count() as u64 >= max)",".is_some_and(|max| us.iter().filter(|u| u.ts >= start).count() as u64 > max)","max_uses: fronteira"),
("L21",L,"u.ts >= start).count()","u.ts > start).count()","janela: borda fechada"),
("L22",L,"if g.constraints.unit.as_deref() != unit {\n                return false;","if false {\n                return false;","unit divergente aceita"),
("L23",L,"current = grant\n                .parent\n                .as_deref()\n                .and_then(|id| self.grants_by_id.get(id));","current = None;","uso nao debita ancestrais (checagem)"),
("L24",L,"if m == \"use\" {\n                return Err((\"denied\", \"broken\"));","if m == \"use\" {\n                return Err((\"denied\", \"denied\"));","use em broken sem message broken"),
("L25",L,"if checked.get(\"reason\").and_then(Value::as_str) == Some(\"denied_by\") {\n                return Err((\"forbidden\", \"forbidden\"));","if false {\n                return Err((\"forbidden\", \"forbidden\"));","deny do delegante nao barra"),
("L26",L,"if checked.get(\"reason\").and_then(Value::as_str) != Some(\"exhausted\") {","if true {","exhausted passa a barrar a delegacao"),
("L27",L,"if delegable && self.depth(parent) >= 1 {\n                return Err((\"invalid_params\"","if false {\n                return Err((\"invalid_params\"","delegable sem profundidade restante aceito"),
("L28",L,"if n > 2 {\n                break;","if n > 5 {\n                break;","depth: guarda de ciclo (equivalente provavel)"),
]
env=dict(os.environ,CARGO_TARGET_DIR=TD)
ONLY=os.environ.get("MUT_ONLY","").split(",") if os.environ.get("MUT_ONLY") else None
for mid,f,a,b,desc in M:
    if ONLY and mid not in ONLY: continue
    p=f"{W}/{f}"; orig=open(p).read()
    if orig.count(a)!=1: print(mid,"TRECHO-NAO-UNICO",orig.count(a),flush=True); continue
    open(p,"w").write(orig.replace(a,b,1)); t=time.time()
    r=subprocess.run(["cargo","test","--offline","-p","grantd","--no-fail-fast"],cwd=W,env=env,capture_output=True,text=True)
    out=r.stdout+r.stderr
    st="NAO-COMPILA" if ("could not compile" in out and "test result" not in out) else ("SOBREVIVEU" if r.returncode==0 else "MORTO")
    print(mid,st,desc,"%ds"%(time.time()-t),flush=True)
    open(p,"w").write(orig)
print("FIM")
