"""Portao de mutacao do sink tmux do chatd (v1.9). Uso: mutacao_sink.py DIR_TRABALHO DIR_TARGET
Copia o workspace, aplica UM mutante por vez em sink_tmux.rs e roda so os testes v19."""
import os, shutil, subprocess, sys, time
SRC=os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))); W=sys.argv[1]; TD=sys.argv[2]
if os.path.exists(W): shutil.rmtree(W)
os.makedirs(W)
shutil.copy(f"{SRC}/Cargo.toml",W); shutil.copy(f"{SRC}/Cargo.lock",W); shutil.copy(f"{SRC}/rust-toolchain.toml",W)
os.makedirs(f"{W}/crates")
shutil.copytree(f"{SRC}/crates/chatd",f"{W}/crates/chatd")
shutil.copytree(f"{SRC}/crates/agentd",f"{W}/crates/agentd",ignore=shutil.ignore_patterns("target"))
shutil.copytree(f"{SRC}/crates/attest",f"{W}/crates/attest")
F="crates/chatd/src/sink_tmux.rs"
M=[
("M01","const CLIENT_LIMIT: usize = 8;","const CLIENT_LIMIT: usize = 9;","limite de clientes 9"),
("M02",'"10000",\n                    "-l",\n','"10000",\n',"display-message sem -l (expande # no texto)"),
("M03",'"-l",\n                    "--",\n                    line,','"-l",\n                    line,',"display-message sem --"),
("M04",'"@chatd_stale", "--", status]','"@chatd_stale", status]',"set-option sem --"),
("M05","Duration::from_secs(5);\nconst TOTAL","Duration::from_secs(50);\nconst TOTAL","timeout por chamada 50 s"),
("M06","TOTAL_LIMIT: Duration = Duration::from_secs(10)","TOTAL_LIMIT: Duration = Duration::from_secs(100)","timeout total 100 s"),
("M07","const OUTPUT_LIMIT: usize = 64 * 1024;","const OUTPUT_LIMIT: usize = 64 * 1024 * 1024;","limite de saida 64 MiB"),
("M08","if joined.len() <= 200 {","if joined.len() <= 201 {","corte em 201"),
("M09",'part == ".."','false',"aceita segmento .."),
("M10","!new_lines.is_empty()","true","list-clients mesmo sem aviso novo"),
("M11",".take(CLIENT_LIMIT)\n","\n","sem limite de clientes"),
("M12","if *kind == \"unaccepted\" { 'p' } else { 'r' }","if *kind == \"unaccepted\" { 'r' } else { 'p' }","p e r trocados"),
("M13","if warned.insert(reason) {","if true {","aviso repetido por chamada"),
("M14","(*minutes).max(0)","*minutes","minutos negativos"),
("M15","name.len() <= 64\n","name.len() <= 65\n","nome de cliente 65"),
("M16","rows.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(b.1)));","","sem ordenar por nome"),
("M17","if Instant::now() >= call_deadline {\n        return Err(SinkError::Timeout);\n    }","","sem checar prazo total antes de chamar"),
("M18",'"set-option", "-g", "@chatd_stale"','"set-option", "@chatd_stale"',"set-option sem -g"),
]
env=dict(os.environ,CARGO_TARGET_DIR=TD)
orig=open(f"{W}/{F}").read()
res=[]
for mid,a,b,desc in M:
    if orig.count(a)!=1: res.append((mid,"TRECHO-NAO-UNICO",desc)); print(mid,"TRECHO-NAO-UNICO",orig.count(a),flush=True); continue
    open(f"{W}/{F}","w").write(orig.replace(a,b,1))
    t=time.time()
    p=subprocess.run(["cargo","test","--offline","-p","chatd","--test","v19_sink_tmux","--test","v19_sink_limites"],cwd=W,env=env,capture_output=True,text=True)
    out=p.stdout+p.stderr
    if "could not compile" in out or "error[" in out: st="NAO-COMPILA"
    elif p.returncode==0: st="SOBREVIVEU"
    else: st="MORTO"
    res.append((mid,st,desc)); print(mid,st,desc,"%ds"%(time.time()-t),flush=True)
open(f"{W}/{F}","w").write(orig)
print("FIM",flush=True)
