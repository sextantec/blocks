#!/usr/bin/env python3
"""Portao de mutacao manual do chatd (nao ha cargo-mutants neste ambiente).

Copia a crate para um diretorio de trabalho, aplica UM mutante por vez (troca textual exata,
a substituicao precisa casar exatamente uma vez), roda `cargo test --offline` e classifica:
  MORTO       = algum teste falhou (bom)
  SOBREVIVEU  = todos os testes passaram (lacuna de teste ou mutante equivalente: analisar)
  NAO-COMPILA = o mutante nao compila (descartado)
Uso: mutacao.py --work DIR --target-dir DIR [--only ID,ID] [--list]
"""
import argparse, json, os, shutil, subprocess, sys, time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# (id, arquivo, trecho original EXATO e unico, substituto, o que o mutante quebra)
M = [
 ("S01","src/store.rs","if seq != expected {","if seq > expected {","seq fora de ordem para tras deixa de ser recusado"),
 ("S02","src/store.rs",'unwrap_or_else(|| "done".to_string())','unwrap_or_else(|| "failed".to_string())',"result padrao do ack deixa de ser done"),
 ("S03","src/store.rs","State::Aceito => return Ok((m, true)),","State::Aceito => return Ok((m, false)),","aceito repetido deixa de ser duplicate"),
 ("S04","src/store.rs","if m.to != by {","if false {","ack por quem nao e destinatario deixa de ser forbidden"),
 ("S05","src/store.rs","if m.result.as_deref() == Some(r.as_str()) {","if true {","resultado diferente vira duplicate"),
 ("S06","src/store.rs","if m.hash == hash {","if true {","envelope diferente com mesmo id vira duplicate"),
 ("S07","src/store.rs","if !self.recipients.contains_key(&p.to) {","if false {","destinatario desconhecido aceito no send"),
 ("S08","src/store.rs","&& !self.msgs.contains_key(t)","&& false","thread inexistente aceito"),
 ("S09","src/store.rs","m.to == to && m.seq > after &&","m.to == to && m.seq >= after &&","after_seq vira inclusivo"),
 ("S10","src/store.rs","(!unresolved_only || m.state != State::Resultado)","(!unresolved_only || m.state != State::Aceito)","unresolved_only esconde aceito em vez de resultado"),
 ("S11","src/store.rs","v.sort_by_key(|m| m.seq);","v.sort_by_key(|m| std::cmp::Reverse(m.seq));","list em ordem decrescente"),
 ("S12","src/store.rs",".skip(after as usize)",".skip(after as usize + 1)","events_after pula um evento"),
 ("S13","src/store.rs",".and_then(|()| self.log.sync_data())",".and_then(|()| Ok(()))","sem fsync antes de responder (observavel so com queda de energia)"),
 ("S14","src/store.rs","f.set_len(at as u64)","f.set_len(0)","truncamento da cauda apaga o log inteiro"),
 ("S15","src/store.rs","                m.accepted_at = Some(ts);\n                if note.is_some() {\n                    m.note = note;\n                }\n                let to = m.to.clone();\n                self.outstanding.remove(&to);","                m.accepted_at = Some(ts);\n                if note.is_some() {\n                    m.note = note;\n                }\n                let to = m.to.clone();\n                let _ = &to;","aceitar nao libera a campainha"),
 ("S16","src/store.rs","&& now.duration_since(*t) < BELL_FAIL_INTERVAL","&& now.duration_since(*t) >= BELL_FAIL_INTERVAL","limite de 1 bell_failed por minuto invertido"),
 ("S17","src/store.rs","|t| now.duration_since(*t) >= rearm","|t| now.duration_since(*t) > rearm","rearme exatamente no limite (fronteira)"),
 ("S18","src/store.rs","**bell\n                    && self.pending_count(name) > 0","**bell\n                    && self.pending_count(name) >= 0","campainha considerada devida sem pendencia"),
 ("S19","src/store.rs","} else if self.outstanding.contains_key(name) {","} else if false {","status nunca mostra outstanding"),
 ("S20","src/store.rs","State::Registrado => pending += 1,","State::Registrado => pending += 0,","status nao conta pendentes"),
 ("S21","src/store.rs",'ev.insert("note".into(), json!(p.note));','ev.insert("note".into(), Value::Null);',"note do ack descartada"),
 ("S22","src/store.rs","if self.msgs.contains_key(id) {\n                    return Err(format!(\"id '{id}' registrado duas vezes\"));","if false {\n                    return Err(format!(\"id '{id}' registrado duas vezes\"));","log com id repetido e aceito na releitura"),
 ("S23","src/store.rs","if is_last {\n                        cut = Some(pos);","if true {\n                        cut = Some(pos);","linha invalida no meio descartada em silencio"),
 ("S24","src/store.rs","if rc != 0 {","if false {","trava exclusiva ignorada"),
 ("S25","src/store.rs",".mode(0o600)",".mode(0o644)","log/trava criados com permissao frouxa"),
 ("S26","src/store.rs","fs::set_permissions(path, fs::Permissions::from_mode(0o600))","fs::set_permissions(path, fs::Permissions::from_mode(0o666))","permissao reajustada para 0666"),
 ("P01","src/protocol.rs","(1..=32).contains(&s.len())","(1..=33).contains(&s.len())","nome aceita 33 caracteres"),
 ("P02","src/protocol.rs","(1..=128).contains(&s.len())","(1..=129).contains(&s.len())","id aceita 129 caracteres"),
 ("P03","src/protocol.rs","matches!(b, b'.' | b'_' | b':' | b'/' | b'-')","matches!(b, b'.' | b'_' | b':' | b'-')","id recusa '/'"),
 ("P04","src/protocol.rs","!(1..=MAX_SUBJECT).contains(&n)","!(0..=MAX_SUBJECT).contains(&n)","subject vazio aceito"),
 ("P06","src/protocol.rs","if body.len() > MAX_BODY {","if body.len() >= MAX_BODY {","body exatamente no limite recusado"),
 ("P07","src/protocol.rs","Some(n) if n.chars().count() <= MAX_NOTE && !has_control","Some(n) if n.chars().count() < MAX_NOTE && !has_control","note exatamente no limite recusada"),
 ("P08","src/protocol.rs","unwrap_or(LIST_DEFAULT);\n    if !(1..=LIST_MAX).contains(&limit) {","unwrap_or(LIST_DEFAULT);\n    if !(1..LIST_MAX).contains(&limit) {","limit 200 recusado no list"),
 ("P09","src/protocol.rs","unwrap_or(LIST_DEFAULT)","unwrap_or(LIST_MAX)","limit padrao vira 200"),
 ("P10","src/protocol.rs","if stage != \"resultado\" {\n                return Err(ApiError::invalid(\n                    \"/result\",\n                    \"result so vale com stage resultado\",","if false {\n                return Err(ApiError::invalid(\n                    \"/result\",\n                    \"result so vale com stage resultado\",","result aceito com stage aceito"),
 ("P11","src/protocol.rs","if k != \"v\" && !allowed.contains(&k.as_str()) {","if false {","campos desconhecidos aceitos"),
 ("P12","src/protocol.rs","if obj.get(\"jsonrpc\").and_then(Value::as_str) != Some(\"2.0\") {","if false {","jsonrpc errado aceito"),
 ("P13","src/protocol.rs","Some(v @ (Value::Number(_) | Value::String(_))) => v.clone(),","Some(v @ (Value::Number(_) | Value::String(_) | Value::Null)) => v.clone(),","id nulo aceito na requisicao"),
 ("P14","src/protocol.rs","pub const MAX_LINE: usize = 1_048_576;","pub const MAX_LINE: usize = 2_097_152;","limite de linha dobrado"),
 ("P15","src/protocol.rs",'"RESPOSTA", "ACK"]','"RESPOSTA", "ACKX"]',"kind ACK deixa de existir"),
 ("B01","src/bell.rs",'Ok(s) if s != "idle" && s != "done" => return,','Ok(s) if s != "idle" => return,',"done nao toca"),
 ("B02","src/bell.rs",'Ok(s) if s != "idle" && s != "done" => return,','Ok(s) if s != "idle" && s != "done" && s != "working" => return,',"working toca"),
 ("B03","src/bell.rs","if n == 0 {\n        return;","if false {\n        return;","toca sem pendencia"),
 ("B04","src/bell.rs",'.pointer("/result/agent/agent_status")','.pointer("/agent_status")',"caminho do estado errado"),
 ("B05","src/bell.rs","Err(reason) => return record_failed(shared, name, reason),","Err(_) => return,","falha do agent get nao vira bell_failed"),
 ("B06","src/bell.rs","if !status.success() {\n        return Err(\"exit_status\");","if false {\n        return Err(\"exit_status\");","saida nao zero tratada como sucesso"),
 ("B07","src/bell.rs",'format!("[chat] {n} pendente(s). Rode: chatd inbox --take")','format!("[chat] {n} pendente(s). Rode: chatd inbox")',"texto da campainha sem --take"),
 ("V01","src/server.rs","if active.load(Ordering::SeqCst) >= MAX_CONNS {","if active.load(Ordering::SeqCst) > MAX_CONNS {","teto de conexoes com folga de uma"),
 ("V02","src/server.rs","const IDLE: Duration = Duration::from_secs(30);","const IDLE: Duration = Duration::from_secs(300);","ociosidade de 300 s"),
 ("V03","src/server.rs","if last > head {","if last >= head {","last_seq igual ao head recusado"),
 ("V04","src/server.rs","if cursor.is_some() {\n                return Err(ApiError::new(\n                    \"invalid_request\",\n                    \"events.resume ja foi chamado nesta conexao\",","if false {\n                return Err(ApiError::new(\n                    \"invalid_request\",\n                    \"events.resume ja foi chamado nesta conexao\",","segundo events.resume aceito"),
 ("V05","src/server.rs","fs::set_permissions(socket, fs::Permissions::from_mode(0o600))","fs::set_permissions(socket, fs::Permissions::from_mode(0o666))","socket 0666"),
 ("V06","src/server.rs","if content.len() > MAX_LINE {","if content.len() >= MAX_LINE {","linha exatamente no limite recusada"),
 # --- v1.1 e secao F (auth, take, refs, denied, controles, escape no cliente) ---
 ("A01","src/auth.rs","t.len() == 64\n        && t.bytes()","t.len() >= 64\n        && t.bytes()","token mais longo que 64 aceito"),
 ("A02","src/auth.rs","acc | (x ^ y)","acc & (x ^ y)","comparacao de hash sempre igual (token errado aceito)"),
 ("A03","src/auth.rs","if obj.len() != 2 ||","if false ||","auth.json com campo extra aceito"),
 ("A04","src/auth.rs",".create_new(true)\n            .mode(0o600)\n            .open(&tmp)?;",".create_new(true)\n            .mode(0o644)\n            .open(&tmp)?;","auth.json temporario com permissao frouxa"),
 ("A05","src/auth.rs",'&& !o.flag("rotate")',"&& false","token existente sobrescrito sem --rotate"),
 ("A06","src/auth.rs","if tokens.remove(&name).is_none() {","if tokens.remove(&name).is_some() {","revoke com sentido invertido"),
 ("A07","src/auth.rs",'None => println!("{token}"),','None => println!("ok {name}"),',"token nao impresso sem --out"),
 ("A08","src/auth.rs","let _ = fs::remove_file(p);","let _ = ();","token orfao em --out quando auth.json nao grava"),
 ("A09","src/auth.rs",'.unwrap_or_else(|| "0".repeat(64));','.unwrap_or_else(|| "a".repeat(64));',"hash de nome inexistente fixo (observavel so no tempo)"),
 ("V10","src/server.rs","const AUTH_MAX_FAILS: usize = 5;","const AUTH_MAX_FAILS: usize = 6;","trava por pid exige 6 falhas"),
 ("V11","src/server.rs","if pid_locked(shared, conn.peer_pid, now) {","if false {","pid trancado ainda tenta autenticar"),
 ("V12","src/server.rs",".remove(&conn.peer_pid);",".get(&conn.peer_pid).map(|_| ());","sucesso nao limpa historico de falhas"),
 ("V13","src/server.rs","let close = now_locked || conn.failures >= AUTH_MAX_FAILS;","let close = now_locked;","conexao nao fecha apos 5 falhas"),
 ("V14","src/server.rs",'if e.code == "forbidden" {\n                record_denied','if false {\n                record_denied',"forbidden sem rastro denied"),
 ("V15","src/server.rs",'"message.list" | "message.take" => "to",','"message.list" => "to",',"denied de take sem claimed"),
 ("V16","src/server.rs",".filter(|s| protocol::valid_name(s));",".filter(|_| true);","nome invalido gravado em denied"),
 ("V17","src/server.rs","if shared.cfg.auth_required && who.is_none() {\n        return Ok(());","if false {\n        return Ok(());","eventos empurrados a conexao nao autenticada"),
 ("V18","src/server.rs","let visible = who.is_none_or(|w| st.visible(&ev, w));","let visible = true;","evento de outro agente visivel no watch"),
 ("V19","src/server.rs","let logged = protocol::valid_name(&name).then_some(name.as_str());","let logged = Some(name.as_str());","nome invalido gravado em auth_failed"),
 ("V20","src/server.rs","if conn.identity.is_some() {\n        let e = ApiError::new(\"invalid_request\", \"auth ja feito","if false {\n        let e = ApiError::new(\"invalid_request\", \"auth ja feito","segundo auth aceito na conexao"),
 ("V21","src/server.rs","if conn.identity.is_none() {\n            return (\n                error_reply(req.id.clone(), &ApiError::unauthenticated()),","if false {\n            return (\n                error_reply(req.id.clone(), &ApiError::unauthenticated()),","metodo sem auth aceito com auth required"),
 ("V22","src/server.rs",'"message.take" => {\n            let tp = protocol::parse_take(p)?;\n            let to = protocol::resolve(tp.to.clone(), who, "/to")?;\n            protocol::check_name(&to, "/to")?;\n            if !shared.cfg.recipients.contains_key(&to) {','"message.take" => {\n            let tp = protocol::parse_take(p)?;\n            let to = protocol::resolve(tp.to.clone(), who, "/to")?;\n            protocol::check_name(&to, "/to")?;\n            if false {',"take de destinatario desconhecido aceito"),
 ("V23","src/server.rs","if m.len() <= PID_CAP {\n        return;","if true {\n        return;","mapa de pids sem teto"),
 ("V24","src/server.rs","locked || !e.fails.is_empty()","true","mapa de pids nunca poda"),
 ("V25","src/server.rs","if e.fails.len() >= AUTH_MAX_FAILS {\n        e.fails.clear();","if e.fails.len() > AUTH_MAX_FAILS {\n        e.fails.clear();","trava do pid so na 6a falha"),
 ("V26","src/server.rs","if !binding::verify(&shared.cfg, name, mux, &params.pane, peer_pid) {","if false {","bind sem verificar que o painel pertence ao processo"),
 ("V27","src/binding.rs","if Some(agent_type) != expected || same_path(cwd, &peer_cwd).is_none() {","if false || same_path(cwd, &peer_cwd).is_none() {","bind ignora o tipo configurado do agente herdr"),
 ("V28","src/store.rs",".filter(|m| m.from == who && m.to != who)?;",".filter(|m| m.to != who)?;","watch --receipts remove o filtro from=identidade"),
 ("V29","src/store.rs",".filter(|m| m.from == who || m.to == who)",".filter(|_| true)","message.get revela mensagem alheia"),
 ("T01","src/store.rs","m.to == to && m.state == State::Registrado","m.state == State::Registrado","take pega mensagens de outro destinatario"),
 ("T02","src/store.rs","sel.truncate(limit);","sel.truncate(limit + 1);","take passa do limite"),
 ("T03","src/store.rs","        sel.sort();\n        sel.truncate","        sel.sort_by(|a, b| b.cmp(a));\n        sel.truncate","take em ordem decrescente"),
 ("T04","src/store.rs",'ev.insert("note".into(), json!("take"));','ev.insert("note".into(), json!("x"));',"note do take diferente de take"),
 ("T05","src/store.rs","if let Some(c) = claimed {","if let Some(c) = claimed.filter(|_| false) {","denied sem claimed"),
 ("T06","src/store.rs","            _ => false,","            _ => true,","eventos de seguranca (denied/auth_failed) visiveis a agente"),
 ("T07","src/store.rs","obj.get(\"from\").and_then(Value::as_str) == Some(who)\n                    || obj.get(\"to\").and_then(Value::as_str) == Some(who)","obj.get(\"from\").and_then(Value::as_str) == Some(who)","destinatario nao ve o registered"),
 ("T08","src/store.rs",".is_some_and(|m| m.from == who || m.to == who)",".is_some_and(|m| m.from == who)","destinatario nao ve accepted/resulted"),
 ("P20","src/protocol.rs","c.is_control() && !allow.contains(&c)","c.is_ascii_control() && !allow.contains(&c)","controle C1 passa"),
 ("P21","src/protocol.rs","has_control(body, &['\\n', '\\t'])","has_control(body, &['\\n', '\\t', '\\r'])","body aceita \\r"),
 ("P22","src/protocol.rs","has_control(subject, &[])","has_control(subject, &['\\t'])","subject aceita tab"),
 ("P23","src/protocol.rs","!has_control(n, &['\\t'])","!has_control(n, &[])","note recusa tab"),
 ("P24","src/protocol.rs","&& !has_control(s, &[]) =>","&& true =>","ref com controle aceita"),
 ("P25","src/protocol.rs","if arr.len() > MAX_REFS {","if arr.len() >= MAX_REFS {","16 refs recusadas"),
 ("P26","src/protocol.rs","(Some(w), Some(f)) if f == w => Ok(f),","(Some(w), Some(f)) if f == w || true => Ok(f),","from diferente da identidade aceito"),
 ("P27","src/protocol.rs",'(None, None) => Err(ApiError::invalid(path, "campo obrigatorio")),','(None, None) => Ok(String::new()),',"campo ausente sem identidade aceito"),
 ("P28","src/protocol.rs","unwrap_or(TAKE_DEFAULT as u64)","unwrap_or(LIST_MAX)","limit padrao do take vira 200"),
 ("P29","src/protocol.rs","    let limit = opt_u64(p, \"limit\")?.unwrap_or(TAKE_DEFAULT as u64);\n    if !(1..=LIST_MAX).contains(&limit) {","    let limit = opt_u64(p, \"limit\")?.unwrap_or(TAKE_DEFAULT as u64);\n    if !(0..=LIST_MAX).contains(&limit) {","take com limit 0 aceito"),
 ("C01","src/client.rs","if c.is_control() {","if c.is_ascii_control() && c != '\\u{7f}' {","escape do cliente deixa passar DEL e C1"),
 ("C02",'src/client.rs','format!("\\\\x{:02x}", c as u32)','format!("\\\\x{:x}", c as u32)',"escape sem zero a esquerda"),
 ("C03","src/client.rs",'if ty == "auth_failed" || ty == "denied" {','if ty == "auth_failed" {',"denied sem marca !! no log"),
 ("C04","src/client.rs",'if take && (o.flag("all") || o.get("after").is_some()) {',"if false {","--take combinado com --all/--after aceito"),
 ("C05","src/client.rs",'"to": name, "limit": 200}))','"to": name, "limit": 1}))',"take do cliente pega 1 por vez"),
 ("C06","src/client.rs",'s.push_str(&format!(" | {}", esc(subj)));','s.push_str(&format!(" | {}", subj));',"assunto sem escape no chatd log"),
 ("C07","src/client.rs","s.push_str(&esc(id));","s.push_str(id);","id sem escape no chatd log"),
 ("C08","src/client.rs","        && tok.is_some()\n        && env != name","        && env != name","watch sem token recusa --as diferente de CHATD_AS"),
 ("G01","src/config.rs",'None | Some("required") => true,','None | Some("required") => false,',"auth padrao vira off"),
 # --- v1.2 (watch robusto) ---
 ("W01","src/client.rs","    if !(1..=86400).contains(&n) {","    if !(0..=86400).contains(&n) {","--exit-after 0 aceito"),
 ("W02","src/client.rs",'"m" => 60,','"m" => 6,',"minuto vale 6 s"),
 ("W03","src/client.rs","Some(n) if n > head => {","Some(n) if n >= head => {","cursor igual ao head recusado"),
 ("W04","src/client.rs","if resume_file.is_some() && from_start {","if false {","--resume-file com --from-start aceito"),
 ("W05","src/client.rs","persist_cursor(st, seq)?;","let _ = seq;","cursor nunca gravado"),
 ("W06","src/client.rs",".create_new(true)\n            .mode(0o600)",".create_new(true)\n            .mode(0o644)","arquivos da v1.2 com permissao frouxa"),
 ("W07","src/client.rs","const HEARTBEAT: Duration = Duration::from_secs(5);","const HEARTBEAT: Duration = Duration::from_secs(60);","heartbeat a cada 60 s"),
 ("W08","src/client.rs","st.backoff = if w >= 16 { 30 } else { w * 2 };","st.backoff = if w >= 16 { 30 } else { w * 3 };","backoff triplica"),
 ("W09","src/client.rs","let start = if st.from_start { 0 } else { head };","let start = 0;","sem arquivo e sem --from-start comeca do zero"),
 ("W10","src/client.rs","                } else {\n                    1\n                };","                } else {\n                    5\n                };","espera fixa sem --reconnect vira 5 s"),]

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--work")
    ap.add_argument("--target-dir")
    ap.add_argument("--only", default="")
    ap.add_argument("--list", action="store_true")
    a = ap.parse_args()
    if a.list:
        for m in M: print(m[0], m[1], "|", m[4])
        return
    if not a.work or not a.target_dir:
        ap.error("--work e --target-dir sao obrigatorios (exceto com --list)")
    only = set(filter(None, a.only.split(",")))
    work = os.path.abspath(a.work)
    if os.path.exists(work): shutil.rmtree(work)
    os.makedirs(work)
    for n in ("src", "tests"):
        # shutil.copy (nao copy2): mtime novo, senao o cargo reaproveita o binario de um mutante anterior
        shutil.copytree(os.path.join(REPO, n), os.path.join(work, n), copy_function=shutil.copy)
    ROOT = os.path.dirname(os.path.dirname(REPO))  # raiz do workspace (Cargo.lock e toolchain)
    for n in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
        src = os.path.join(REPO, n)
        if not os.path.exists(src):
            src = os.path.join(ROOT, n)
        shutil.copy(src, work)
    env = dict(os.environ, CARGO_TARGET_DIR=os.path.abspath(a.target_dir))
    env["PATH"] = os.path.expanduser("~/.cargo/bin") + os.pathsep + env["PATH"]
    def cargo_test():
        t = time.time()
        p = subprocess.run(["cargo", "test", "--offline", "-q"], cwd=work, env=env,
                           capture_output=True, text=True, timeout=900)
        return p.returncode, p.stdout + p.stderr, time.time() - t
    rc, out, dt = cargo_test()
    if rc != 0:
        print("BASE NAO PASSA; abortando", out[-1500:]); sys.exit(2)
    print(f"base verde em {dt:.0f}s")
    res = []
    for mid, f, old, new, why in M:
        if only and mid not in only: continue
        path = os.path.join(work, f)
        orig = open(path).read()
        if orig.count(old) != 1:
            print(f"{mid}: trecho original casa {orig.count(old)} vezes; ajustar o mutante"); res.append((mid, "INVALIDO", why)); continue
        open(path, "w").write(orig.replace(old, new))
        try:
            rc, out, dt = cargo_test()
        finally:
            open(path, "w").write(orig)
        if rc == 0: st = "SOBREVIVEU"
        elif "error[E" in out or "could not compile" in out: st = "NAO-COMPILA"
        else: st = "MORTO"
        print(f"{mid} {st:11} {dt:5.0f}s  {why}", flush=True)
        res.append((mid, st, why))
    c = {}
    for _, s, _ in res: c[s] = c.get(s, 0) + 1
    print("RESUMO", json.dumps(c))
    for mid, s, why in res:
        if s in ("SOBREVIVEU", "NAO-COMPILA", "INVALIDO"): print("ATENCAO", mid, s, why)

if __name__ == "__main__":
    main()
