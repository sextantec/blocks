//! Testes de caixa preta de configuracao, arquivo de regras e CLI (spec rev5, secoes 6, 7 e 8; Anexos
//! A e B): R11 (arquivo de regras recusado e `agentd rules check`), R12 (limites da configuracao) e
//! codigos de saida da CLI. O tmux e FALSO quando precisa. Hoje devem FALHAR (binario da rev4).

mod rev5_common;

use rev5_common::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Diretorio temporario com HOME proprio para as execucoes do binario (apagado ao sair do escopo).
fn home_tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tmp")
}

/// Controle positivo: a configuracao `texto` tem que SUBIR (socket aberto). Sem isto, uma recusa
/// poderia passar por acidente (um binario que recusa tudo passaria em todo teste de recusa).
fn aceita_config(texto: &str) -> Result<(), String> {
    let dir = tempfile::tempdir().expect("tmp");
    let cfgp = dir.path().join("config.json");
    fs::write(&cfgp, texto).expect("gravar config");
    let estado = dir.path().join("estado");
    fs::create_dir_all(&estado).unwrap();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sobe(&estado, &cfgp)));
    match r {
        Ok(_srv) => Ok(()),
        Err(_) => Err("configuracao VALIDA de controle foi recusada".into()),
    }
}

/// Exige: `base` sobe (controle) e `texto` (a mutacao) e recusada com saida 1 e socket ausente.
fn deve_recusar_config(base: &str, texto: &str) -> Result<(), String> {
    aceita_config(base).map_err(|e| format!("controle: {e}"))?;
    let dir = tempfile::tempdir().expect("tmp");
    let cfgp = dir.path().join("config.json");
    fs::write(&cfgp, texto).expect("gravar config");
    let estado = dir.path().join("estado");
    fs::create_dir_all(&estado).unwrap();
    let home = home_tmp();
    let cfg_s = cfgp.display().to_string();
    let est_s = estado.display().to_string();
    let r = roda(
        home.path(),
        None,
        &["serve", "--config", &cfg_s, "--state-dir", &est_s],
        Duration::from_secs(10),
    );
    if r.code != Some(1) {
        return Err(format!(
            "saida {:?} (esperado 1); stderr: {}",
            r.code,
            r.stderr.trim()
        ));
    }
    if estado.join("agentd.sock").exists() {
        return Err("socket aberto apesar da recusa".into());
    }
    Ok(())
}

/// Configuracao valida de referencia, como texto JSON, para mutar com `Value`.
fn cfg_ref(m: &Mundo, regras: &Path) -> Value {
    cfg(m, regras, 200, &[("claude", "claude", "claude")])
}

// ---------- R12: limites e recusas da configuracao ----------

#[test]
fn r12_configuracoes_invalidas_nao_sobem() {
    let m = Mundo::novo();
    let regras = m.raiz.path().join("regras.json");
    copia_regras(&regras, &regras_padrao());
    let base = cfg_ref(&m, &regras);
    // Cada caso: nome e funcao que muta a configuracao de referencia.
    type Mut = Box<dyn Fn(&mut Value)>;
    let casos: Vec<(&str, Mut)> = vec![
        (
            "type herdr recusado",
            Box::new(|c: &mut Value| c["agents"]["claude"]["sources"][0]["type"] = json!("herdr")),
        ),
        (
            "fonte herdr com campo pane",
            Box::new(|c: &mut Value| {
                c["agents"]["claude"]["sources"] = json!([{"type": "herdr", "pane": "w1:p1"}])
            }),
        ),
        (
            "campo desconhecido no topo",
            Box::new(|c: &mut Value| c["herdr_bin"] = json!("herdr")),
        ),
        (
            "campo desconhecido no agente",
            Box::new(|c: &mut Value| c["agents"]["claude"]["extra"] = json!(1)),
        ),
        (
            "kind fora da lista",
            Box::new(|c: &mut Value| c["agents"]["claude"]["kind"] = json!("foo")),
        ),
        (
            "pane que nao e %N",
            Box::new(|c: &mut Value| {
                c["agents"]["claude"]["sources"] = json!([{"type": "tmux", "pane": "w1:p1"}])
            }),
        ),
        (
            "tag fora de [a-z0-9_-]",
            Box::new(|c: &mut Value| {
                c["agents"]["claude"]["sources"] = json!([{"type": "tmux", "tag": "Bad Tag"}])
            }),
        ),
        (
            "id fora de [a-z0-9_-]",
            Box::new(|c: &mut Value| {
                let v = c["agents"]["claude"].clone();
                c["agents"] = json!({"Claude Bad": v});
            }),
        ),
        (
            "sources com dois elementos",
            Box::new(|c: &mut Value| {
                c["agents"]["claude"]["sources"] =
                    json!([{"type": "tmux", "tag": "claude"}, {"type": "tmux", "tag": "x"}])
            }),
        ),
        (
            "sources vazio",
            Box::new(|c: &mut Value| c["agents"]["claude"]["sources"] = json!([])),
        ),
        (
            "tag repetida entre agentes",
            Box::new(|c: &mut Value| {
                c["agents"]["outro"] =
                    json!({"kind": "claude", "sources": [{"type": "tmux", "tag": "claude"}]});
            }),
        ),
        (
            "pane repetido entre agentes",
            Box::new(|c: &mut Value| {
                c["agents"]["claude"]["sources"] = json!([{"type": "tmux", "pane": "%1"}]);
                c["agents"]["outro"] =
                    json!({"kind": "claude", "sources": [{"type": "tmux", "pane": "%1"}]});
            }),
        ),
        (
            "poll_ms abaixo de 200",
            Box::new(|c: &mut Value| c["poll_ms"] = json!(100)),
        ),
        (
            "poll_ms acima de 10000",
            // Isolado: stale e safety ficam validos para 10001, so o poll viola o limite.
            Box::new(|c: &mut Value| {
                c["poll_ms"] = json!(10001);
                c["safety_poll_ms"] = json!(20000);
                c["stale_after_ms"] = json!(30000);
            }),
        ),
        (
            "stale_after_ms <= poll_ms + source_timeout_ms",
            Box::new(|c: &mut Value| c["stale_after_ms"] = json!(600)),
        ),
        (
            "gone_after_s abaixo de 5",
            Box::new(|c: &mut Value| c["gone_after_s"] = json!(4)),
        ),
        (
            "idle_confirm_polls zero",
            Box::new(|c: &mut Value| c["idle_confirm_polls"] = json!(0)),
        ),
        (
            "ring abaixo de 64",
            Box::new(|c: &mut Value| c["ring"] = json!(63)),
        ),
        (
            "mais de 32 agentes",
            Box::new(|c: &mut Value| {
                let mut ags = serde_json::Map::new();
                for i in 0..33 {
                    ags.insert(format!("a{i}"), json!({"kind": "claude", "sources": [{"type": "tmux", "tag": format!("t{i}")}]}));
                }
                c["agents"] = Value::Object(ags);
            }),
        ),
        (
            "tmux_bin vazio",
            Box::new(|c: &mut Value| c["tmux_bin"] = json!("")),
        ),
        (
            "tmux_bin que nao e string",
            Box::new(|c: &mut Value| c["tmux_bin"] = json!(5)),
        ),
        (
            "tmux_socket relativo",
            Box::new(|c: &mut Value| c["tmux_socket"] = json!("relativo/sock")),
        ),
        (
            "events nao booleano",
            Box::new(|c: &mut Value| c["events"] = json!("sim")),
        ),
        (
            "event_debounce_ms abaixo de 50",
            Box::new(|c: &mut Value| c["event_debounce_ms"] = json!(10)),
        ),
        (
            "safety_poll_ms menor que poll_ms",
            Box::new(|c: &mut Value| {
                c["poll_ms"] = json!(2000);
                c["safety_poll_ms"] = json!(1000);
            }),
        ),
        (
            "rules_file ausente",
            Box::new(|c: &mut Value| {
                c.as_object_mut().unwrap().remove("rules_file");
            }),
        ),
    ];
    let mut falhas = Vec::new();
    for (nome, mutar) in casos {
        let mut c = base.clone();
        mutar(&mut c);
        if let Err(e) = deve_recusar_config(&base.to_string(), &c.to_string()) {
            falhas.push(format!("  {nome}: {e}"));
        }
    }
    assert!(
        falhas.is_empty(),
        "R12: configuracoes que deviam ser recusadas subiram ou saiu errado:\n{}",
        falhas.join("\n")
    );
}

#[test]
fn r12_chave_duplicada_na_configuracao_e_recusada() {
    let m = Mundo::novo();
    let regras = m.raiz.path().join("regras.json");
    copia_regras(&regras, &regras_padrao());
    let texto = cfg_ref(&m, &regras).to_string();
    let duplicado = texto.replacen("{", "{\"poll_ms\": 300, ", 1);
    deve_recusar_config(&texto, &duplicado).expect("R12: chave duplicada deve ser recusada");
}

#[test]
fn r12_fonte_por_pane_e_aceita_e_funciona() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let regras = m.raiz.path().join("regras.json");
    copia_regras(&regras, &regras_padrao());
    let mut c = cfg(&m, &regras, 200, &[]);
    c["agents"] =
        json!({"claude": {"kind": "claude", "sources": [{"type": "tmux", "pane": "%1"}]}});
    let cfgp = escreve_cfg(m.raiz.path(), &c);
    let srv = sobe(&estado_dir(&m), &cfgp);
    let a = espera_estado(&srv.socket, "claude", "idle", Duration::from_secs(6));
    assert_eq!(
        a["state"], "idle",
        "R12: fonte por pane devia funcionar: {a}"
    );
}

// ---------- R11: arquivo de regras ----------

/// Texto de `default.json` com uma mutacao textual, gravado em copia 0644. Devolve o caminho.
fn regras_mutadas(dir: &Path, texto: &str) -> PathBuf {
    let p = dir.join("regras.json");
    fs::write(&p, texto).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
    p
}

fn texto_padrao() -> String {
    fs::read_to_string(regras_padrao()).unwrap()
}

fn json_padrao() -> Value {
    serde_json::from_str(&texto_padrao()).unwrap()
}

#[test]
fn r11_arquivo_de_regras_invalido_nao_sobe() {
    let m = Mundo::novo();
    let mut falhas = Vec::new();
    let dir = m.raiz.path().to_path_buf();

    let mut casos: Vec<(&str, String, Option<u32>)> = Vec::new();
    // Campo desconhecido no topo.
    casos.push((
        "campo desconhecido",
        texto_padrao().replacen("{", "{\"extra\": 1, ", 1),
        None,
    ));
    // Chave duplicada (schema repetido).
    casos.push((
        "chave duplicada",
        texto_padrao().replacen("{", "{\"schema\": 1, ", 1),
        None,
    ));
    // Regex invalida.
    casos.push((
        "regex invalida",
        texto_padrao().replacen("\"re\": \"Esc to cancel\"", "\"re\": \"Esc to cancel(\"", 1),
        None,
    ));
    // Campo desconhecido numa regra.
    {
        let mut v = json_padrao();
        v["agents"]["claude"]["rules"][0]["extra"] = json!(true);
        casos.push(("campo desconhecido na regra", v.to_string(), None));
    }
    // kind da configuracao sem entrada no arquivo (o arquivo nao tem `pi`).
    {
        let mut v = json_padrao();
        v["agents"].as_object_mut().unwrap().remove("pi");
        casos.push(("kind sem regras", v.to_string(), Some(0)));
    }
    // Regra sem fixture.
    {
        let mut v = json_padrao();
        v["agents"]["claude"]["rules"][2]["fixtures"] = json!([]);
        casos.push(("regra sem fixture", v.to_string(), None));
    }
    // Blocked sem reason.
    {
        let mut v = json_padrao();
        v["agents"]["claude"]["rules"][1]
            .as_object_mut()
            .unwrap()
            .remove("reason");
        casos.push(("blocked sem reason", v.to_string(), None));
    }
    // Reason presente em working.
    {
        let mut v = json_padrao();
        v["agents"]["claude"]["rules"][4]["reason"] = json!("permission");
        casos.push(("reason em working", v.to_string(), None));
    }

    // Controle usa uma copia limpa (regras-base.json); as mutantes vao para regras.json.
    let base_rules = dir.join("regras-base.json");
    copia_regras(&base_rules, &regras_padrao());
    let base = cfg(&m, &base_rules, 200, &[("claude", "claude", "claude")]);
    for (nome, texto, _) in casos {
        let p = regras_mutadas(&dir, &texto);
        let mut c = cfg(&m, &p, 200, &[("claude", "claude", "claude")]);
        if nome == "kind sem regras" {
            c["agents"]["claude"]["kind"] = json!("pi");
            c["agents"]["claude"]["sources"] = json!([{"type": "tmux", "tag": "claude"}]);
        }
        if let Err(e) = deve_recusar_config(&base.to_string(), &c.to_string()) {
            falhas.push(format!("  {nome}: {e}"));
        }
    }

    // Permissao 0666: grupo e outros gravam => nao sobe (controle: a mesma config com 0644 sobe).
    {
        let p = regras_mutadas(&dir, &texto_padrao());
        let c = cfg(&m, &p, 200, &[("claude", "claude", "claude")]);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o666)).unwrap();
        if let Err(e) = deve_recusar_config(&base.to_string(), &c.to_string()) {
            falhas.push(format!("  arquivo gravavel por outros (0666): {e}"));
        }
    }
    assert!(
        falhas.is_empty(),
        "R11: arquivo de regras que devia ser recusado:\n{}",
        falhas.join("\n")
    );
}

// ---------- R11: agentd rules check ----------

fn fixtures_s() -> String {
    fixtures().display().to_string()
}

#[test]
fn r11_rules_check_passa_no_arquivo_padrao_com_saida_0() {
    let home = home_tmp();
    let r = roda(
        home.path(),
        None,
        &[
            "rules",
            "check",
            "--rules",
            &regras_padrao().display().to_string(),
            "--fixtures",
            &fixtures_s(),
        ],
        Duration::from_secs(20),
    );
    assert_eq!(
        r.code,
        Some(0),
        "R11: rules check do arquivo padrao devia sair 0: {} {}",
        r.stdout,
        r.stderr
    );
    let j = roda(
        home.path(),
        None,
        &[
            "rules",
            "check",
            "--rules",
            &regras_padrao().display().to_string(),
            "--fixtures",
            &fixtures_s(),
            "--json",
        ],
        Duration::from_secs(20),
    );
    assert_eq!(
        j.code,
        Some(0),
        "R11: rules check --json tambem devia sair 0"
    );
}

#[test]
fn r11_rules_check_falha_se_uma_regra_nao_tem_fixture_que_a_case_e_nao_imprime_tela() {
    let dir = tempfile::tempdir().unwrap();
    // Trust: a fixture declarada passa a ser a tela ociosa; nenhuma fixture casa `claude.blocked.trust`.
    let mut v = json_padrao();
    v["agents"]["claude"]["rules"][2]["fixtures"] = json!(["claude/idle-manual"]);
    let p = regras_mutadas(dir.path(), &v.to_string());
    let home = home_tmp();
    let r = roda(
        home.path(),
        None,
        &[
            "rules",
            "check",
            "--rules",
            &p.display().to_string(),
            "--fixtures",
            &fixtures_s(),
        ],
        Duration::from_secs(20),
    );
    assert_eq!(
        r.code,
        Some(1),
        "R11: regra sem fixture que a case devia sair 1: {} {}",
        r.stdout,
        r.stderr
    );
    // Uma linha de conteudo da tela de trust, que nao aparece no arquivo de regras, nao pode vazar.
    let trust = String::from_utf8_lossy(&tela_fixture("claude", "trust")).into_owned();
    let regras_txt = texto_padrao();
    let linha = trust
        .lines()
        .map(|l| l.trim())
        .find(|l| l.chars().count() >= 14 && !regras_txt.contains(l))
        .expect("linha distintiva da tela trust");
    let saida = todo_texto(&r);
    assert!(
        !saida.contains(linha),
        "R11: a mensagem do rules check imprimiu conteudo da tela"
    );
}

#[test]
fn r11_rules_check_uso_incorreto_sai_2() {
    let home = home_tmp();
    // Controle: a forma correta sai 0 (senao um binario sem `rules check` passaria por acidente).
    let ok = roda(
        home.path(),
        None,
        &[
            "rules",
            "check",
            "--rules",
            &regras_padrao().display().to_string(),
            "--fixtures",
            &fixtures_s(),
        ],
        Duration::from_secs(20),
    );
    assert_eq!(
        ok.code,
        Some(0),
        "R11: controle: rules check valido devia sair 0"
    );
    let sem_rules = roda(
        home.path(),
        None,
        &["rules", "check", "--fixtures", &fixtures_s()],
        Duration::from_secs(10),
    );
    assert_eq!(sem_rules.code, Some(2), "R11: falta --rules devia sair 2");
    let sem_valor = roda(
        home.path(),
        None,
        &[
            "rules",
            "check",
            "--rules",
            &regras_padrao().display().to_string(),
            "--fixtures",
        ],
        Duration::from_secs(10),
    );
    assert_eq!(
        sem_valor.code,
        Some(2),
        "R11: --fixtures sem valor devia sair 2"
    );
}

// ---------- CLI: codigos de saida e formato ----------

/// Spec 15.1: `get --json` => `data = {"agent", "epoch", "head_seq"}`.
fn json_do_objeto(v: &Value) -> &Value {
    &v["data"]["agent"]
}

/// Spec 15.1: `list --json` => `data = {"agents": [...], "epoch", "head_seq"}`.
fn json_lista(v: &Value) -> Vec<Value> {
    v["data"]["agents"].as_array().cloned().unwrap_or_default()
}

#[test]
fn cli_codigos_de_saida_e_formato_com_daemon_no_ar() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.tela_fixture("%1", "claude", "box-draft");
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(
        espera_estado(&srv.socket, "claude", "idle", Duration::from_secs(6))["state"] == "idle"
    );
    let home = home_tmp();
    let mut falhas = Vec::new();

    let ok = cli(home.path(), &srv.socket, &["get", "claude"]);
    if ok.code != Some(0) {
        falhas.push(format!("get claude: saida {:?}", ok.code));
    }
    let linha = ok.stdout.trim();
    if !linha.starts_with("claude ") || !linha.contains("idle") {
        falhas.push(format!(
            "get sem --json: uma linha 'ID ESTADO...' esperada, veio {linha:?}"
        ));
    }

    let ojson = cli(home.path(), &srv.socket, &["get", "claude", "--json"]);
    match serde_json::from_str::<Value>(&ojson.stdout) {
        Ok(v) => {
            if v["schema_version"] != 1 || v["ok"] != true {
                falhas.push(format!("get --json: envelope {v}"));
            }
            if !v["data"]["epoch"].is_string() || v["data"]["head_seq"].as_u64().is_none() {
                falhas.push(format!(
                    "get --json: data sem epoch (string) e head_seq: {v}"
                ));
            }
            let ag = json_do_objeto(&v);
            if !ag.is_object() {
                falhas.push(format!("get --json: data.agent ausente: {v}"));
            }
            if ag.get("rule").is_none() || ag.get("input").is_none() {
                falhas.push(format!("get --json: faltam campos rule/input: {ag}"));
            }
            if ag["input"] != "draft" {
                falhas.push(format!(
                    "get --json: input esperado draft, veio {}",
                    ag["input"]
                ));
            }
        }
        Err(e) => falhas.push(format!("get --json nao e JSON: {e}")),
    }

    let lj = cli(home.path(), &srv.socket, &["list", "--json"]);
    match serde_json::from_str::<Value>(&lj.stdout) {
        Ok(v) => {
            if !v["data"]["epoch"].is_string() || v["data"]["head_seq"].as_u64().is_none() {
                falhas.push(format!("list --json: data sem epoch e head_seq: {v}"));
            }
            let ags = json_lista(&v);
            if ags.len() != 1 || ags[0].get("rule").is_none() {
                falhas.push(format!("list --json: esperado 1 agente com rule: {v}"));
            }
        }
        Err(e) => falhas.push(format!("list --json nao e JSON: {e}")),
    }
    let l = cli(home.path(), &srv.socket, &["list"]);
    if l.code != Some(0) || l.stdout.lines().count() != 1 {
        falhas.push(format!(
            "list sem --json: uma linha por agente, saida {:?}: {:?}",
            l.code, l.stdout
        ));
    }

    let nao = cli(home.path(), &srv.socket, &["get", "nao-existe"]);
    if nao.code != Some(3) {
        falhas.push(format!(
            "agente inexistente: saida {:?} (esperado 3)",
            nao.code
        ));
    }
    let nao_j = cli(home.path(), &srv.socket, &["get", "nao-existe", "--json"]);
    if let Ok(v) = serde_json::from_str::<Value>(&nao_j.stdout)
        && (v["ok"] != false || v["code"] != "unknown_agent")
    {
        falhas.push(format!("agente inexistente --json: {v}"));
    }

    let h = cli(home.path(), &srv.socket, &["health"]);
    if h.code != Some(0) {
        falhas.push(format!(
            "health com fonte ok: saida {:?} (esperado 0)",
            h.code
        ));
    }
    let st = cli(home.path(), &srv.socket, &["status", "--json"]);
    if st.code != Some(0) {
        falhas.push(format!("status --json: saida {:?}", st.code));
    }

    let since = cli(home.path(), &srv.socket, &["watch", "--since", "1"]);
    if since.code != Some(2) {
        falhas.push(format!(
            "watch --since sem --epoch: saida {:?} (esperado 2)",
            since.code
        ));
    }
    let epoch = cli(home.path(), &srv.socket, &["watch", "--epoch", "1"]);
    if epoch.code != Some(2) {
        falhas.push(format!(
            "watch --epoch sem --since: saida {:?} (esperado 2)",
            epoch.code
        ));
    }

    assert!(falhas.is_empty(), "CLI:\n{}", falhas.join("\n"));
}

#[test]
fn cli_sem_nenhum_socket_sai_2_e_daemon_fora_sai_4() {
    let home = home_tmp();
    let sem = roda(
        home.path(),
        None,
        &["get", "claude"],
        Duration::from_secs(10),
    );
    assert_eq!(
        sem.code,
        Some(2),
        "CLI: sem --socket, AGENTD_SOCKET nem --state-dir devia sair 2"
    );
    let fora = roda(
        home.path(),
        None,
        &[
            "get",
            "claude",
            "--socket",
            "/nonexistent-dir-rev5/agentd.sock",
        ],
        Duration::from_secs(10),
    );
    assert_eq!(fora.code, Some(4), "CLI: daemon indisponivel devia sair 4");
}

#[test]
fn cli_health_sai_1_quando_o_list_panes_falha() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.listar("falha");
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(
        espera_fonte(&srv.socket, false, Duration::from_secs(5)),
        "a fonte devia falhar"
    );
    let home = home_tmp();
    let h = cli(home.path(), &srv.socket, &["health"]);
    assert_eq!(
        h.code,
        Some(1),
        "CLI: health com a execucao da fonte falha devia sair 1"
    );
}

#[test]
fn cli_health_nao_falha_por_agente_unknown_com_fonte_ok() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.tela("%1", b"tela sem regra nenhuma\n");
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(espera_fonte(&srv.socket, true, Duration::from_secs(5)));
    let a = espera_agente(&srv.socket, "claude", Duration::from_secs(4), |x| {
        !x["observed_at"].is_null()
    });
    assert_eq!(a["state"], "unknown", "{a}");
    let home = home_tmp();
    let h = cli(home.path(), &srv.socket, &["health"]);
    assert_eq!(
        h.code,
        Some(0),
        "CLI: health depende so da execucao da fonte, nao do estado do agente"
    );
}

// ---------- spec 15: tmux_bin padrao, fixtures e prontidao ----------

#[test]
fn r12_tmux_bin_ausente_usa_o_tmux_do_path() {
    // Spec 15.4: sem `tmux_bin` vale "tmux" procurado no PATH. O PATH do daemon so tem o tmux falso.
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "claude").tag("claude")]);
    m.tela_fixture("%1", "claude", "box-empty");
    let pathdir = m.raiz.path().join("bin-do-path");
    fs::create_dir_all(&pathdir).unwrap();
    let alvo = pathdir.join("tmux");
    fs::copy(&m.tmux, &alvo).unwrap();
    fs::set_permissions(&alvo, fs::Permissions::from_mode(0o755)).unwrap();
    let regras = m.raiz.path().join("regras.json");
    copia_regras(&regras, &regras_padrao());
    let mut c = cfg(&m, &regras, 200, &[("claude", "claude", "claude")]);
    c.as_object_mut().unwrap().remove("tmux_bin");
    let cfgp = escreve_cfg(m.raiz.path(), &c);
    let path_s = pathdir.display().to_string();
    let srv = sobe_com(None, &estado_dir(&m), &cfgp, &[("PATH", path_s.as_str())]);
    let a = espera_estado(&srv.socket, "claude", "idle", Duration::from_secs(6));
    assert_eq!(
        a["state"], "idle",
        "R12 (spec 15.4): sem tmux_bin devia usar o tmux do PATH: {a}"
    );
    assert!(
        !m.chamadas().is_empty(),
        "R12: o tmux do PATH nao foi chamado"
    );
}

#[test]
fn r11_fixtures_nao_sao_abertas_pelo_serve_mas_rules_check_nomeia_a_faltante() {
    // Spec 15.5: serve so confere que `fixtures` nao e vazia; rules check abre cada fixture.
    let m = Mundo::novo();
    let mut v = json_padrao();
    v["agents"]["claude"]["rules"][2]["fixtures"] = json!(["claude/nao-existe-rev5"]);
    let p = m.raiz.path().join("regras-fixtures.json");
    fs::write(&p, v.to_string()).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
    let c = cfg(&m, &p, 200, &[("claude", "claude", "claude")]);
    if let Err(e) = aceita_config(&c.to_string()) {
        panic!("R11 (spec 15.5): serve nao devia abrir as fixtures: {e}");
    }
    let home = home_tmp();
    let r = roda(
        home.path(),
        None,
        &[
            "rules",
            "check",
            "--rules",
            &p.display().to_string(),
            "--fixtures",
            &fixtures_s(),
        ],
        Duration::from_secs(20),
    );
    assert_eq!(
        r.code,
        Some(1),
        "R11 (spec 15.5): fixture inexistente devia sair 1: {}",
        todo_texto(&r)
    );
    assert!(
        todo_texto(&r).contains("claude/nao-existe-rev5"),
        "R11 (spec 15.5): a saida devia nomear a fixture: {}",
        todo_texto(&r)
    );
}

#[test]
fn cli_serve_imprime_pronto_quando_aceita_conexao() {
    // Spec 15.10 (rev4 secao 13.6): linha `agentd: pronto` em stderr quando o socket aceita conexao.
    let m = Mundo::novo();
    let srv = inicia(&m, 200, &[("claude", "claude", "claude")], |_| {});
    assert!(
        espera(Duration::from_secs(5), || srv
            .stderr()
            .lines()
            .any(|l| l.trim() == "agentd: pronto")),
        "R14/CLI (spec 15.10): faltou a linha 'agentd: pronto' em stderr: {}",
        srv.stderr()
    );
}
