//! Testes de caixa preta da descoberta de agentes (spec rev5.3: secao 16 e R21 a R27 da secao 11).
//! O tmux e FALSO (fake_tmux.py). Cada teste sobe o agentd com `discover: true` (ou sem ele, para o padrao).
//! Ids e origens sao os da secao 16. Hoje devem FALHAR contra a rev5.2 (sem descoberta).
//!
//! Ambiguidades (secao 16 nao fixa) NAO sao testadas aqui; estao listadas em REV5-TESTES.md.

mod rev5_common;

use rev5_common::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

/// Sobe o daemon com `discover: true`; `ajuste` altera a config (ex.: fonte cwd, events, forget).
fn sobe_desc(
    m: &Mundo,
    agentes: &[(&str, &str, &str)],
    ajuste: impl FnOnce(&mut Value),
) -> Servidor {
    inicia(m, 200, agentes, move |c| {
        c["discover"] = json!(true);
        ajuste(c);
    })
}

/// `agent.get` sem panico: None se o id nao existe (`unknown_agent`).
fn ag(socket: &Path, id: &str) -> Option<Value> {
    let r = rpc(socket, "agent.get", json!({"v": 1, "id": id}));
    if is_err(&r) {
        None
    } else {
        Some(res(&r)["agent"].clone())
    }
}

/// Espera o id aparecer em `agent.get` e devolve o Agente.
fn espera_id(socket: &Path, id: &str, d: Duration) -> Value {
    espera(d, || ag(socket, id).is_some());
    ag(socket, id).unwrap_or_else(|| panic!("agente {id} nao apareceu em {d:?}"))
}

fn ids(socket: &Path) -> BTreeSet<String> {
    lista(socket)
        .iter()
        .map(|a| a["id"].as_str().unwrap_or("?").to_string())
        .collect()
}

fn conj(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn origem(a: &Value) -> String {
    a["origin"].as_str().unwrap_or("<sem origin>").to_string()
}

fn id_valido(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn descoberta(socket: &Path) -> Value {
    status(socket)["sources"]["tmux"]["discover"].clone()
}

// ---------- R21: descoberta basica e o padrao ----------

#[test]
fn r21_descoberta_ligada_acha_claude_codex_e_pi_com_origin_discovered() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").cwd("/home/x/horus-reality-check"),
        Painel::new("%2", "codex").cwd("/home/x/blocks"),
        Painel::new("%3", "pi").cwd("/home/x/pi-proj"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.tela_fixture("%2", "codex", "idle");
    m.tela_fixture("%3", "pi", "idle");
    let srv = sobe_desc(&m, &[], |_| {});
    let c = espera_id(
        &srv.socket,
        "claude-horus-reality-check",
        Duration::from_secs(6),
    );
    assert_eq!(origem(&c), "discovered", "R21: {c}");
    assert_eq!(c["state"], "idle", "R21: estado pelas regras: {c}");
    let x = espera_id(&srv.socket, "codex-blocks", Duration::from_secs(6));
    assert_eq!(origem(&x), "discovered", "R21: {x}");
    let p = espera_id(&srv.socket, "pi-pi-proj", Duration::from_secs(6));
    assert_eq!(origem(&p), "discovered", "R21: {p}");
    assert_eq!(
        ids(&srv.socket),
        conj(&["claude-horus-reality-check", "codex-blocks", "pi-pi-proj"]),
        "R21: so os tres agentes descobertos"
    );
}

#[test]
fn r21_descoberta_desligada_nao_mostra_nem_captura_painel_sem_marca() {
    // discover ausente = padrao false (spec 16): nenhum painel fora de `agents` aparece e nenhum e capturado.
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "codex").cwd("/w/blocks")]);
    m.tela_fixture("%1", "codex", "idle");
    // agents so pode ser vazio com discover true (spec 16): aqui ha um configurado, sem painel.
    let srv = inicia(&m, 200, &agentes_todos()[..1], |_| {});
    assert!(
        espera(Duration::from_secs(5), || m.listagens() >= 4),
        "R21: poucos ciclos"
    );
    // Controle positivo: o status ja traz o objeto discover (spec 16); sem ele este teste passaria por acidente.
    assert!(
        descoberta(&srv.socket).is_object(),
        "R21: status.sources.tmux.discover ausente"
    );
    assert_eq!(
        ids(&srv.socket),
        conj(&["claude"]),
        "R21: com discover false so o configurado aparece"
    );
    assert!(
        !m.capturas().iter().any(|c| c == "%1"),
        "R21: painel sem marca capturado com discover false: {:?}",
        m.capturas()
    );
    assert_eq!(
        descoberta(&srv.socket)["on"],
        false,
        "R21: status.discover.on deve ser false"
    );
}

#[test]
fn r21_shell_vim_desconhecido_e_morto_nunca_viram_agente() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "zsh").cwd("/w/a"),
        Painel::new("%2", "vim").cwd("/w/b"),
        Painel::new("%3", "node").cwd("/w/c"),
        Painel::new("%4", "codex").cwd("/w/d").morto(true),
    ]);
    let srv = sobe_desc(&m, &[], |_| {});
    assert!(
        espera(Duration::from_secs(5), || m.listagens() >= 4),
        "R21: poucos ciclos"
    );
    assert!(
        ids(&srv.socket).is_empty(),
        "R21: shell/vim/desconhecido/morto viraram agente: {:?}",
        ids(&srv.socket)
    );
    for p in ["%1", "%2", "%3", "%4"] {
        assert!(
            !m.capturas().iter().any(|c| c == p),
            "R21: {p} capturado sem ser agente"
        );
    }
}

// ---------- R22: nome estavel ----------

#[test]
fn r22_id_fixo_mesmo_com_diretorio_mudando_no_cenario() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "codex").cwd("/w/blocks")]);
    m.tela_fixture("%1", "codex", "idle");
    let srv = sobe_desc(&m, &[], |_| {});
    espera_id(&srv.socket, "codex-blocks", Duration::from_secs(6));
    m.paineis(&[Painel::new("%1", "codex").cwd("/w/outro-lugar")]);
    let fim = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < fim {
        assert_eq!(
            ids(&srv.socket),
            conj(&["codex-blocks"]),
            "R22: o id mudou com o diretorio"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        origem(&ag(&srv.socket, "codex-blocks").expect("codex-blocks")),
        "discovered"
    );
}

#[test]
fn r22_dois_ou_tres_paineis_no_mesmo_diretorio_sufixo_pela_ordem_numerica_do_pane_id() {
    // %10 e %9 : a ordem e numerica (9 antes de 10), nao textual.
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%10", "codex").cwd("/w/blocks"),
        Painel::new("%9", "codex").cwd("/w/blocks"),
        Painel::new("%2", "codex").cwd("/w/blocks"),
    ]);
    m.tela_fixture("%2", "codex", "idle");
    m.tela_fixture("%9", "codex", "working-indicator");
    m.tela_fixture("%10", "codex", "menu-model");
    let srv = sobe_desc(&m, &[], |_| {});
    assert_eq!(
        ids(&srv.socket),
        conj(&["codex-blocks", "codex-blocks-2", "codex-blocks-3"]),
        "R22: tres paineis no mesmo diretorio"
    );
    // %2 -> codex-blocks (idle), %9 -> -2 (working), %10 -> -3 (blocked): confere pelo estado de cada tela.
    let a = espera_estado(&srv.socket, "codex-blocks", "idle", Duration::from_secs(6));
    assert_eq!(
        a["state"], "idle",
        "R22: codex-blocks devia ser o %2 (idle): {a}"
    );
    let b = espera_estado(
        &srv.socket,
        "codex-blocks-2",
        "working",
        Duration::from_secs(6),
    );
    assert_eq!(
        b["state"], "working",
        "R22: codex-blocks-2 devia ser o %9 (working): {b}"
    );
    let c = espera_estado(
        &srv.socket,
        "codex-blocks-3",
        "blocked",
        Duration::from_secs(6),
    );
    assert_eq!(
        c["state"], "blocked",
        "R22: codex-blocks-3 devia ser o %10 (blocked): {c}"
    );
}

#[test]
fn r22_o_segundo_nao_rouba_o_id_quando_o_primeiro_some_e_reaparece() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "codex").cwd("/w/blocks"),
        Painel::new("%2", "codex").cwd("/w/blocks"),
    ]);
    m.tela_fixture("%1", "codex", "idle");
    m.tela_fixture("%2", "codex", "working-indicator");
    let srv = sobe_desc(&m, &[], |_| {});
    espera_estado(
        &srv.socket,
        "codex-blocks-2",
        "working",
        Duration::from_secs(6),
    );
    // %1 some: codex-blocks vai a gone (gone_after_s 5). %2 continua.
    m.paineis(&[Painel::new("%2", "codex").cwd("/w/blocks")]);
    let g = espera_estado(&srv.socket, "codex-blocks", "gone", Duration::from_secs(12));
    assert_eq!(
        g["state"], "gone",
        "R22: o painel que sumiu devia ir a gone: {g}"
    );
    // %1 reaparece com o mesmo pane_id e pid: volta a ser codex-blocks, e %2 segue codex-blocks-2.
    m.paineis(&[
        Painel::new("%1", "codex").cwd("/w/blocks"),
        Painel::new("%2", "codex").cwd("/w/blocks"),
    ]);
    let a = espera_estado(&srv.socket, "codex-blocks", "idle", Duration::from_secs(8));
    assert_eq!(
        a["state"], "idle",
        "R22: o primeiro devia voltar com o mesmo id: {a}"
    );
    assert_eq!(
        ids(&srv.socket),
        conj(&["codex-blocks", "codex-blocks-2"]),
        "R22: o reaparecido nao pode criar um terceiro id nem roubar o do segundo"
    );
    let b = agente_id(&srv.socket, "codex-blocks-2");
    assert_eq!(
        b["state"], "working",
        "R22: codex-blocks-2 devia continuar no %2 (working): {b}"
    );
}

fn agente_id(socket: &Path, id: &str) -> Value {
    ag(socket, id).unwrap_or_else(|| panic!("agente {id} ausente"))
}

#[test]
fn r22_troca_de_pane_pid_no_mesmo_pane_id_recalcula_o_id() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "codex").cwd("/w/blocks")]);
    m.tela_fixture("%1", "codex", "idle");
    let srv = sobe_desc(&m, &[], |_| {});
    espera_id(&srv.socket, "codex-blocks", Duration::from_secs(6));
    // Mesmo pane_id, pid novo, outro diretorio: painel novo (regra 3 da rev4), id recalculado.
    m.paineis(&[Painel::new("%1", "codex").pid("5555").cwd("/w/novo")]);
    let n = espera_id(&srv.socket, "codex-novo", Duration::from_secs(8));
    assert_eq!(origem(&n), "discovered", "R22: {n}");
}

// ---------- R23: precedencia e origem ----------

#[test]
fn r23_configurado_por_cwd_vence_e_nunca_e_descoberto_de_novo() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "codex").cwd("/w/blocks")]);
    m.tela_fixture("%1", "codex", "idle");
    let srv = sobe_desc(&m, &[], |c| agente_cwd(c, "dev", "codex", "/w/blocks"));
    let d = espera_id(&srv.socket, "dev", Duration::from_secs(6));
    assert_eq!(
        origem(&d),
        "configured",
        "R23: fonte cwd devia dar origin configured: {d}"
    );
    // Espera uma janela de varios ciclos: nenhum id descoberto pode aparecer para o mesmo painel.
    let c0 = m.listagens();
    assert!(
        espera(Duration::from_secs(5), || m.listagens() >= c0 + 3),
        "R23: poucos ciclos"
    );
    assert_eq!(
        ids(&srv.socket),
        conj(&["dev"]),
        "R23: o painel configurado nao pode ser descoberto de novo"
    );
}

#[test]
fn r23_marca_sem_entrada_vira_agente_com_origin_mark() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "codex").tag("nomex").cwd("/w/blocks")]);
    m.tela_fixture("%1", "codex", "idle");
    let srv = sobe_desc(&m, &[], |_| {});
    let a = espera_id(&srv.socket, "nomex", Duration::from_secs(6));
    assert_eq!(origem(&a), "mark", "R23: {a}");
    assert_eq!(
        ids(&srv.socket),
        conj(&["nomex"]),
        "R23: a marca define o nome, nao o diretorio"
    );
}

#[test]
fn r23_marca_off_exclui_o_painel_da_descoberta() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "codex").tag("off").cwd("/w/blocks")]);
    m.tela_fixture("%1", "codex", "idle");
    let srv = sobe_desc(&m, &[], |_| {});
    assert!(
        espera(Duration::from_secs(5), || m.listagens() >= 4),
        "R23: poucos ciclos"
    );
    assert!(
        ids(&srv.socket).is_empty(),
        "R23: @agentd off devia excluir o painel: {:?}",
        ids(&srv.socket)
    );
    assert!(
        !m.capturas().iter().any(|c| c == "%1"),
        "R23: painel com marca off capturado"
    );
}

#[test]
fn r23_descoberto_que_colide_com_configurado_leva_sufixo_mesmo_com_pane_menor() {
    // %1 (menor pane_id) e descoberto e colide com o id configurado `codex-blocks`; %2 e o configurado.
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "codex").cwd("/w/blocks"),
        Painel::new("%2", "codex")
            .tag("codex-blocks")
            .cwd("/w/blocks"),
    ]);
    m.tela_fixture("%1", "codex", "idle");
    m.tela_fixture("%2", "codex", "idle");
    let srv = sobe_desc(&m, &agentes_todos()[1..2], |_| {});
    let c = espera_id(&srv.socket, "codex-blocks", Duration::from_secs(6));
    assert_eq!(
        origem(&c),
        "configured",
        "R23: o configurado nunca e tomado: {c}"
    );
    let d = espera_id(&srv.socket, "codex-blocks-2", Duration::from_secs(6));
    assert_eq!(
        origem(&d),
        "discovered",
        "R23: o descoberto que colide leva -2: {d}"
    );
}

// ---------- R24: ciclo de vida, esquecimento e teto ----------

#[test]
fn r24_painel_que_some_vai_a_gone_e_e_esquecido_depois_de_discover_forget_s() {
    // Spec 16/R24: discover_forget_s minimo valido = 60 s. Lento de proposito (um teste so).
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "codex").cwd("/w/blocks")]);
    m.tela_fixture("%1", "codex", "idle");
    let srv = sobe_desc(&m, &[], |c| c["discover_forget_s"] = json!(60));
    espera_estado(&srv.socket, "codex-blocks", "idle", Duration::from_secs(6));
    // Volta ao shell: ausente como em R10.
    m.paineis(&[Painel::new("%1", "zsh").cwd("/w/blocks")]);
    let g = espera_estado(&srv.socket, "codex-blocks", "gone", Duration::from_secs(12));
    assert_eq!(g["state"], "gone", "R24: {g}");
    let head = status(&srv.socket)["head_seq"].as_u64().expect("head_seq");
    // Antes de 60 s em gone ainda existe.
    std::thread::sleep(Duration::from_secs(20));
    assert!(
        ag(&srv.socket, "codex-blocks").is_some(),
        "R24: esquecido antes de discover_forget_s"
    );
    // Depois de 60 s em gone: unknown_agent, sem evento extra.
    let esquecido = espera(Duration::from_secs(75), || {
        ag(&srv.socket, "codex-blocks").is_none()
    });
    assert!(esquecido, "R24: nao foi esquecido em ~60 s");
    let evs = eventos_desde(&srv.socket, head);
    assert!(
        !evs.iter().any(|e| e["id"] == "codex-blocks"),
        "R24: esquecer nao gera evento: {evs:?}"
    );
}

#[test]
fn r24_teto_de_32_conta_excedentes_em_ignored() {
    let mut m = Mundo::novo();
    let ps: Vec<Painel> = (1..=33)
        .map(|i| Painel::new(&format!("%{i}"), "codex").cwd(&format!("/w/p{i}")))
        .collect();
    m.paineis(&ps);
    let srv = sobe_desc(&m, &[], |_| {});
    let ok = espera(Duration::from_secs(10), || {
        descoberta(&srv.socket)["ignored"].as_u64() == Some(1)
    });
    assert!(
        ok,
        "R24: status.discover.ignored devia ser 1 (33 candidatos, teto 32): {}",
        descoberta(&srv.socket)
    );
    let lst = lista(&srv.socket);
    assert_eq!(lst.len(), 32, "R24: o teto de 32 agentes deve valer");
    assert!(lst.iter().all(|a| origem(a) == "discovered"));
}

// ---------- R25: captura minima e clientes de eventos ----------

#[test]
fn r25_so_captura_configurado_marcado_ou_descoberto_vivo() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").cwd("/w/a"), // configurado
        Painel::new("%2", "codex").cwd("/w/b"),                // descoberto
        Painel::new("%7", "claude").tag("outro").cwd("/w/g"),  // marcado, sem entrada
        Painel::new("%3", "zsh").cwd("/w/c"),                  // shell
        Painel::new("%4", "vim").cwd("/w/d"),                  // comando que nao e agente
        Painel::new("%5", "codex").cwd("/w/e").morto(true),    // morto
        Painel::new("%6", "codex").tag("off").cwd("/w/f"),     // excluido pela marca
    ]);
    for p in ["%1", "%2", "%7"] {
        m.tela_fixture(p, "claude", "box-empty");
    }
    let srv = sobe_desc(&m, &agentes_todos()[..1], |_| {});
    assert!(
        espera(Duration::from_secs(8), || m.listagens() >= 5),
        "R25: poucos ciclos"
    );
    let caps = m.capturas();
    for permitido in ["%1", "%2", "%7"] {
        assert!(
            caps.iter().any(|c| c == permitido),
            "R25: {permitido} devia ser capturado: {caps:?}"
        );
    }
    for proibido in ["%3", "%4", "%5", "%6"] {
        assert!(
            !caps.iter().any(|c| c == proibido),
            "R25: {proibido} capturado sem ser agente: {caps:?}"
        );
    }
    let _ = srv;
}

#[test]
fn r25_clientes_de_eventos_nascem_para_sessao_descoberta_e_no_maximo_8() {
    let mut m = Mundo::novo();
    let ps: Vec<Painel> = (1..=9)
        .map(|i| {
            Painel::new(&format!("%{i}"), "codex")
                .cwd(&format!("/w/p{i}"))
                .sessao(&format!("${i}"))
        })
        .collect();
    m.paineis(&ps);
    let _srv = sobe_desc(&m, &[], |c| {
        c["events"] = json!(true);
    });
    assert!(
        espera(Duration::from_secs(8), || m.anexos().len() >= 8),
        "R25: clientes de eventos para sessoes com painel descoberto nao subiram"
    );
    std::thread::sleep(Duration::from_millis(800));
    let alvos: BTreeSet<String> = m
        .anexos()
        .iter()
        .map(|a| a.last().cloned().unwrap_or_default())
        .collect();
    assert!(alvos.len() <= 8, "R25: mais de 8 clientes: {alvos:?}");
    assert_eq!(
        alvos.len(),
        m.anexos().len(),
        "R25: dois clientes para a mesma sessao"
    );
    assert!(
        alvos.iter().all(|t| t.starts_with('$')),
        "R25: alvos fora de $N: {alvos:?}"
    );
}

// ---------- R26: caminho hostil ----------

#[test]
fn r26_caminhos_hostis_geram_ids_validos_sem_executar_nem_vazar() {
    // Sentinela so no componente pai: o id deriva so o ultimo componente. Nada pode vazar (R5 estendido).
    let sent = sentinela("PAI-DESC");
    let raiz_pwn = std::env::temp_dir().join(format!("pwn-{}", std::process::id()));
    let casos = [
        format!("/{sent}/x y"),
        format!("/{sent}/a\tb"),
        format!("/{sent}/\"aspas\""),
        format!("/{sent}/$(touch {})", raiz_pwn.display()),
        format!("/{sent}/a\u{e7}\u{e3}o-\u{f1}"),
        format!("/{sent}/{}", "a".repeat(300)),
        "/".to_string(),
    ];
    let mut m = Mundo::novo();
    let ps: Vec<Painel> = casos
        .iter()
        .enumerate()
        .map(|(i, c)| Painel::new(&format!("%{}", i + 1), "codex").cwd(c))
        .collect();
    m.paineis(&ps);
    let srv = sobe_desc(&m, &[], |_| {});
    assert!(espera(Duration::from_secs(8), || lista(&srv.socket).len()
        == casos.len()));
    std::thread::sleep(Duration::from_millis(400));
    let agentes = lista(&srv.socket);
    assert_eq!(agentes.len(), casos.len(), "R26: um agente por caminho");
    for a in &agentes {
        let id = a["id"].as_str().unwrap_or("");
        assert!(id_valido(id), "R26: id invalido: {id:?}");
    }
    assert_eq!(
        fonte_tmux(&srv.socket)["ok"],
        true,
        "R26: a leitura do list-panes nao pode quebrar"
    );
    assert!(!raiz_pwn.exists(), "R26: um shell executou o caminho");
    // Vazamento: a sentinela do pai nao sai em nenhuma saida do daemon.
    let home = tempfile::tempdir().unwrap();
    let saidas = [
        serde_json::to_string(&lista(&srv.socket)).unwrap(),
        serde_json::to_string(&status(&srv.socket)).unwrap(),
        serde_json::to_string(&eventos_todos(&srv.socket)).unwrap(),
        todo_texto(&cli(home.path(), &srv.socket, &["list", "--json"])),
        srv.stderr(),
    ];
    for (i, t) in saidas.iter().enumerate() {
        assert!(
            !t.contains(&sent),
            "R26/R5: o componente pai vazou (saida {i})"
        );
    }
}

#[test]
fn r26_ids_derivados_sao_os_da_spec() {
    // Spec 16: minusculas; fora de [a-z0-9_-] vira '-'; repeticoes de '-' colapsam; apara -/_/. nas pontas;
    // vazio (raiz) => so <kind>; truncado a 32 com o prefixo <kind>- preservado.
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "codex").cwd("/w/Blocks"),
        Painel::new("%2", "codex").cwd("/w/a---b"),
        Painel::new("%3", "codex").cwd("/w/_abc_"),
        Painel::new("%4", "codex").cwd("/"),
        Painel::new("%5", "codex").cwd(&format!("/w/{}", "a".repeat(40))),
        Painel::new("%6", "codex").cwd("/w/x y"),
    ]);
    let srv = sobe_desc(&m, &[], |_| {});
    let mut esperado = conj(&[
        "codex-blocks",
        "codex-a-b",
        "codex-abc",
        "codex",
        "codex-x_y",
    ]);
    esperado.insert(format!("codex-{}", "a".repeat(26)));
    let ok = espera(Duration::from_secs(8), || ids(&srv.socket) == esperado);
    assert!(
        ok,
        "R26: ids derivados diferentes da spec: {:?}",
        ids(&srv.socket)
    );
}

#[test]
fn r26_pane_current_path_vazio_e_source_bad_output() {
    // Spec 16: pane_current_path vazio = leitura falha (source_bad_output), como qualquer linha invalida.
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "codex").cwd("")]);
    let srv = sobe_desc(&m, &[], |_| {});
    let ok = espera(Duration::from_secs(5), || {
        fonte_tmux(&srv.socket)["ok"] == false
    });
    assert!(
        ok,
        "R26: caminho vazio devia falhar a leitura: {}",
        fonte_tmux(&srv.socket)
    );
    assert_eq!(fonte_tmux(&srv.socket)["last_error"], "source_bad_output");
}

// ---------- R27: contrato ----------

#[test]
fn r27_origin_em_get_list_eventos_e_cli() {
    let mut m = Mundo::novo();
    m.paineis(&[
        Painel::new("%1", "claude").tag("claude").cwd("/w/a"),
        Painel::new("%2", "codex").cwd("/w/blocks"),
        Painel::new("%3", "pi").tag("marca").cwd("/w/c"),
    ]);
    m.tela_fixture("%1", "claude", "box-empty");
    m.tela_fixture("%2", "codex", "idle");
    m.tela_fixture("%3", "pi", "idle");
    let srv = sobe_desc(&m, &agentes_todos()[..1], |_| {});
    let c = espera_estado(&srv.socket, "claude", "idle", Duration::from_secs(6));
    assert_eq!(origem(&c), "configured", "R27 agent.get: {c}");
    let d = espera_id(&srv.socket, "codex-blocks", Duration::from_secs(6));
    assert_eq!(origem(&d), "discovered", "R27 agent.get: {d}");
    let k = espera_id(&srv.socket, "marca", Duration::from_secs(6));
    assert_eq!(origem(&k), "mark", "R27 agent.get: {k}");
    let l = lista(&srv.socket);
    let mut origens: Vec<(String, String)> = l
        .iter()
        .map(|a| (a["id"].as_str().unwrap_or("").to_string(), origem(a)))
        .collect();
    origens.sort();
    assert_eq!(
        origens,
        vec![
            ("claude".to_string(), "configured".to_string()),
            ("codex-blocks".to_string(), "discovered".to_string()),
            ("marca".to_string(), "mark".to_string()),
        ],
        "R27 agent.list"
    );
    let evs = eventos_todos(&srv.socket);
    let ev_codex: Vec<&Value> = evs.iter().filter(|e| e["id"] == "codex-blocks").collect();
    assert!(
        !ev_codex.is_empty(),
        "R27: devia haver evento do agente descoberto"
    );
    assert!(
        ev_codex.iter().all(|e| e["origin"] == "discovered"),
        "R27 eventos: {ev_codex:?}"
    );
    let home = tempfile::tempdir().unwrap();
    let g = cli(home.path(), &srv.socket, &["get", "codex-blocks", "--json"]);
    let gj: Value = serde_json::from_str(&g.stdout).unwrap_or(Value::Null);
    assert_eq!(
        gj["data"]["agent"]["origin"], "discovered",
        "R27 CLI get --json: {}",
        g.stdout
    );
    let lj: Value =
        serde_json::from_str(&cli(home.path(), &srv.socket, &["list", "--json"]).stdout)
            .unwrap_or(Value::Null);
    let agentes_cli = lj["data"]["agents"].as_array().cloned().unwrap_or_default();
    assert!(
        agentes_cli
            .iter()
            .any(|a| a["id"] == "codex-blocks" && a["origin"] == "discovered"),
        "R27 CLI list --json: {lj}"
    );
    let linha = cli(home.path(), &srv.socket, &["get", "codex-blocks"]).stdout;
    assert!(
        linha.contains("discovered"),
        "R27: a linha da CLI devia trazer a origem: {linha:?}"
    );
}

#[test]
fn r27_status_discover_tem_on_found_e_ignored() {
    let mut m = Mundo::novo();
    m.paineis(&[Painel::new("%1", "codex").cwd("/w/blocks")]);
    m.tela_fixture("%1", "codex", "idle");
    let srv = inicia(&m, 200, &agentes_todos()[..1], |_| {});
    let d = descoberta(&srv.socket);
    assert_eq!(d["on"], false, "R27: {d}");
    assert_eq!(d["found"], 0, "R27: {d}");
    assert_eq!(d["ignored"], 0, "R27: {d}");

    let mut m2 = Mundo::novo();
    m2.paineis(&[Painel::new("%1", "codex").cwd("/w/blocks")]);
    m2.tela_fixture("%1", "codex", "idle");
    let srv2 = sobe_desc(&m2, &[], |_| {});
    espera_id(&srv2.socket, "codex-blocks", Duration::from_secs(6));
    let d2 = descoberta(&srv2.socket);
    assert_eq!(d2["on"], true, "R27: {d2}");
    assert_eq!(d2["found"], 1, "R27: um descoberto: {d2}");
    assert_eq!(d2["ignored"], 0, "R27: {d2}");
}

/// Config base com discover true e sem agentes (valida pela regra da spec 16: agents vazio so com discover true).
fn cfg_desc(m: &Mundo) -> Value {
    let regras = m.raiz.path().join("regras-desc.json");
    copia_regras(&regras, &regras_padrao());
    let mut c = cfg(m, &regras, 200, &[]);
    c["discover"] = json!(true);
    c
}

/// Sobe a config como texto e confirma que o daemon aceita (controle positivo).
fn aceita(texto: &Value, dir: &Path) -> Result<(), String> {
    let cfgp = escreve_cfg(dir, texto);
    let est = dir.join("estado-aceita");
    std::fs::create_dir_all(&est).unwrap();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sobe(&est, &cfgp)));
    r.map(|_| ())
        .map_err(|_| "configuracao VALIDA de controle foi recusada".to_string())
}

/// Exige saida 1 e socket ausente para a config mutante.
fn recusa(mutante: &Value, dir: &Path, nome: &str) -> Result<(), String> {
    let cfgp = escreve_cfg(dir, mutante);
    let est = dir.join(format!("estado-{nome}"));
    std::fs::create_dir_all(&est).unwrap();
    let home = tempfile::tempdir().unwrap();
    let r = roda(
        home.path(),
        None,
        &[
            "serve",
            "--config",
            &cfgp.display().to_string(),
            "--state-dir",
            &est.display().to_string(),
        ],
        Duration::from_secs(10),
    );
    if r.code != Some(1) {
        return Err(format!(
            "{nome}: saida {:?} (esperado 1); stderr: {}",
            r.code,
            r.stderr.trim()
        ));
    }
    if est.join("agentd.sock").exists() {
        return Err(format!("{nome}: socket aberto apesar da recusa"));
    }
    Ok(())
}

#[test]
fn r27_validacoes_de_descoberta_recusam_saida_1() {
    let m = Mundo::novo();
    let dir = m.raiz.path().to_path_buf();
    let base = cfg_desc(&m);
    aceita(&base, &dir).expect("R27: controle: discover true sem agentes deve subir");

    type Mut = Box<dyn Fn(&mut Value)>;
    let casos: Vec<(&str, Mut)> = vec![
        (
            "discover_forget_s abaixo de 60",
            Box::new(|c: &mut Value| c["discover_forget_s"] = json!(59)),
        ),
        (
            "discover_forget_s acima de 86400",
            Box::new(|c: &mut Value| c["discover_forget_s"] = json!(86401)),
        ),
        (
            "cwd relativo",
            Box::new(|c: &mut Value| agente_cwd(c, "dev", "codex", "relativo/blocks")),
        ),
        (
            "cwd com espaco",
            Box::new(|c: &mut Value| agente_cwd(c, "dev", "codex", "/w/a b")),
        ),
        (
            "cwd com ..",
            Box::new(|c: &mut Value| agente_cwd(c, "dev", "codex", "/w/../x")),
        ),
        (
            "discover nao booleano",
            Box::new(|c: &mut Value| c["discover"] = json!("sim")),
        ),
        (
            "agents vazio com discover false",
            Box::new(|c: &mut Value| c["discover"] = json!(false)),
        ),
    ];
    let mut falhas = Vec::new();
    for (nome, mutar) in casos {
        let mut c = base.clone();
        mutar(&mut c);
        if let Err(e) = recusa(&c, &dir, &nome.replace(' ', "-")) {
            falhas.push(format!("  {e}"));
        }
    }
    assert!(
        falhas.is_empty(),
        "R27: configs que deviam ser recusadas:\n{}",
        falhas.join("\n")
    );
}

#[test]
fn r27_agents_vazio_so_e_aceito_com_discover_true() {
    let m = Mundo::novo();
    let dir = m.raiz.path().to_path_buf();
    let mut c = cfg_desc(&m);
    c["discover"] = json!(false);
    recusa(&c, &dir, "vazio-sem-descoberta")
        .expect("R27: agents vazio com discover false deve ser recusado");
    let mut ok = cfg_desc(&m);
    ok["discover"] = json!(true);
    aceita(&ok, &dir).expect("R27: agents vazio com discover true deve subir");
}
