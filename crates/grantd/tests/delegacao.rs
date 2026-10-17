//! Delegacao e revogacao em cascata (G22 a G27 e G44 a G48), pela biblioteca, com disco e relogio falsos.
//! Perfil: `dono` (owner) e a1, a2, a3 (pedem e delegam). Rev1.2 (5A.8): recurso do filho concreto,
//! grantee do filho concreto, pai com grantee igual ao delegante, `until` do filho obrigatorio.
mod common;

use common::*;
use grantd::Service;
use serde_json::{Value, json};

/// Campos do filho de uma delegacao (`parent` e o delegante saem do chamador).
struct Filho<'a> {
    grantee: &'a str,
    acao: &'a str,
    recurso: &'a str,
    ate: i64,
    extra: Value,
}

fn filho<'a>(grantee: &'a str, acao: &'a str, recurso: &'a str, ate: i64) -> Filho<'a> {
    Filho {
        grantee,
        acao,
        recurso,
        ate,
        extra: json!({}),
    }
}

impl Filho<'_> {
    fn com_extra(mut self, extra: Value) -> Self {
        self.extra = extra;
        self
    }
}

/// `delegate` de `agente` a partir de `pai`. Devolve a resposta crua parseada.
fn delega(svc: &mut Service, agente: &str, pai: &str, f: Filho) -> Value {
    let base = json!({
        "parent": pai,
        "grantee": f.grantee,
        "action": f.acao,
        "resource": f.recurso,
        "until": em(f.ate),
        "request_id": novo_rid(),
    });
    pede_como(svc, agente, "delegate", com(base, f.extra))
}

/// Concessao `allow` do dono, delegavel, para `grantee`, ate `ate` s. `extra` mescla campos (constraints...).
fn concede_delegavel(
    svc: &mut Service,
    grantee: &str,
    acao: &str,
    recurso: &str,
    ate: i64,
    extra: Value,
) -> String {
    let base = com(
        allow(grantee, acao, recurso),
        json!({"until": em(ate), "delegable": true, "request_id": novo_rid()}),
    );
    id_de(&pede_dono(svc, "grant", com(base, extra)))
}

// G22: delegacao acima do pai = exceeds_parent, por acao diferente da do pai (5A.8 mantem "acao igual"),
// recurso fora do padrao, `until` maior e `constraints` maiores (pai com max_uses 2, filho com 3).
// Controle: delegacao contida (recurso concreto dentro do padrao, until menor, constraints menores) passa.
#[test]
fn g22_delegacao_acima_do_pai_e_exceeds_parent() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let pai = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));
    let pai_limitado = concede_delegavel(
        &mut s,
        A1,
        "fs.read",
        "/c/**",
        3600,
        json!({"constraints": {"max_uses": 2}}),
    );

    let acao = delega(&mut s, A1, &pai, filho(A2, "fs.write", "/p/x", 600));
    assert_eq!(codigo(&acao), "exceeds_parent", "acao diferente: {acao}");
    let recurso = delega(&mut s, A1, &pai, filho(A2, "fs.read", "/q/x", 600));
    assert_eq!(
        codigo(&recurso),
        "exceeds_parent",
        "recurso fora: {recurso}"
    );
    let ate = delega(&mut s, A1, &pai, filho(A2, "fs.read", "/p/x", 7200));
    assert_eq!(codigo(&ate), "exceeds_parent", "until maior: {ate}");
    let constr = delega(
        &mut s,
        A1,
        &pai_limitado,
        filho(A2, "fs.read", "/c/x", 600).com_extra(json!({"constraints": {"max_uses": 3}})),
    );
    assert_eq!(
        codigo(&constr),
        "exceeds_parent",
        "constraints maiores: {constr}"
    );

    let ok = delega(&mut s, A1, &pai, filho(A2, "fs.read", "/p/x", 600));
    resultado(&ok);
    assert_decisao(&decide(&mut s, A2, "fs.read", "/p/x"), "allow", "ok");
    let ok_limitado = delega(
        &mut s,
        A1,
        &pai_limitado,
        filho(A2, "fs.read", "/c/x", 600).com_extra(json!({"constraints": {"max_uses": 1}})),
    );
    resultado(&ok_limitado);
}

// G23: pai `permanent` exige `until` finito no filho; a falta e `missing_until` (6A, 5A.8d).
// Controle: com `until` finito, a delegacao passa.
#[test]
fn g23_pai_permanente_exige_until_finito_no_filho() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let pai = id_de(&pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/p/**"),
            json!({"permanent": true, "delegable": true, "request_id": novo_rid()}),
        ),
    ));

    let sem_until = pede_como(
        &mut s,
        A1,
        "delegate",
        json!({"parent": pai, "grantee": A2, "action": "fs.read", "resource": "/p/x", "request_id": novo_rid()}),
    );
    assert_eq!(codigo(&sem_until), "missing_until", "{sem_until}");

    let com_until = delega(&mut s, A1, &pai, filho(A2, "fs.read", "/p/x", 600));
    resultado(&com_until);
    assert_decisao(&decide(&mut s, A2, "fs.read", "/p/x"), "allow", "ok");
}

// G24: profundidade (6A e 5A.8). Dono (prof. 0) -> a1 (g2, prof. 1, delegavel) -> a2 (g3, prof. 2,
// nao delegavel). Delegar a partir de g3 = exceeds_parent, porque o pai nao e delegavel.
// Controles: os dois niveis anteriores passam, e g3 responde allow para a3.
#[test]
fn g24_profundidade_3_e_exceeds_parent() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let g1 = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));

    let g2 = id_de(&delega(
        &mut s,
        A1,
        &g1,
        filho(A2, "fs.read", "/p/x", 1800).com_extra(json!({"delegable": true})),
    ));
    let g3 = id_de(&delega(&mut s, A2, &g2, filho(A3, "fs.read", "/p/x", 900)));
    assert_eq!(grant_de(&decide(&mut s, A3, "fs.read", "/p/x")), json!(g3));

    let profundidade3 = delega(&mut s, A3, &g3, filho(A2, "fs.read", "/p/x", 600));
    assert_eq!(codigo(&profundidade3), "exceeds_parent", "{profundidade3}");
}

// G25: no maximo 3 filhos por concessao (`max_children`). O 4o = quota. Controle: outro pai (dono -> a1)
// ainda aceita delegacao, pois o limite e por concessao.
#[test]
fn g25_quarto_filho_e_quota() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let pai = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));
    let outro_pai = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));

    for i in 1..=3 {
        let v = delega(
            &mut s,
            A1,
            &pai,
            filho(A2, "fs.read", &format!("/p/{i}"), 600),
        );
        resultado(&v);
    }
    let quarto = delega(&mut s, A1, &pai, filho(A2, "fs.read", "/p/4", 600));
    assert_eq!(codigo(&quarto), "quota", "{quarto}");

    let de_outro = delega(&mut s, A1, &outro_pai, filho(A2, "fs.read", "/p/4", 600));
    resultado(&de_outro);
}

// G26: `revoke` do pai revoga filhos e netos no MESMO commit (um tx com 3 eventos `revoke`).
// Cauda cortada no fim do log (o commit do revoke): ao reabrir, nada foi aplicado (a1, a2 e a3 continuam
// com a concessao). Caso completo: tudo revogado. Controle antes do revoke: a3 tem allow pela g3.
#[test]
fn g26_revoke_em_cascata_e_um_commit_so_e_cauda_cortada_nao_aplica() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let perfis = perfis_delegacao();
    let mut s = sobe(&d, &r, &grants_de(&perfis));
    let g1 = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));
    let g2 = id_de(&delega(
        &mut s,
        A1,
        &g1,
        filho(A2, "fs.read", "/p/x", 1800).com_extra(json!({"delegable": true})),
    ));
    let g3 = id_de(&delega(&mut s, A2, &g2, filho(A3, "fs.read", "/p/x", 900)));
    let grants = grants_de(&perfis);

    assert_eq!(grant_de(&decide(&mut s, A3, "fs.read", "/p/x")), json!(g3));
    let head_antes = head(&mut s);
    let bytes_antes = d.bytes();

    resultado(&pede_dono(
        &mut s,
        "revoke",
        json!({"id": g1, "reason": "cascata", "request_id": novo_rid()}),
    ));
    let head_depois = head(&mut s);
    assert_eq!(head_depois, head_antes + 1, "revoke em cascata = um commit");
    let tx = ultimo_tx(&d);
    let revogados: Vec<Value> = tx.iter().map(|e| e["id"].clone()).collect();
    assert_eq!(tx.len(), 3, "um evento por concessao da cascata: {tx:?}");
    assert!(tx.iter().all(|e| e["e"] == json!("revoke")), "{tx:?}");
    for id in [&g1, &g2, &g3] {
        assert!(revogados.contains(&json!(id)), "faltou {id} em {tx:?}");
    }

    // Caso completo reaberto: tudo revogado.
    let completo = d.bytes();
    drop(s);
    let d_completo = Disco::com(&completo);
    let mut s_completo = sobe(&d_completo, &r, &grants);
    for agente in [A1, A2, A3] {
        assert_decisao(
            &decide(&mut s_completo, agente, "fs.read", "/p/x"),
            "deny",
            "no_grant",
        );
    }

    // Cauda cortada: (a) so o '\n' final some; (b) 20 bytes do fim somem.
    for corte in [1usize, 20usize] {
        let cortado = &completo[..completo.len() - corte];
        let d_cortado = Disco::com(cortado);
        let mut s_cortado = sobe(&d_cortado, &r, &grants);
        assert_eq!(
            head(&mut s_cortado),
            head_antes,
            "corte {corte}: nada aplicado"
        );
        assert_eq!(
            d_cortado.bytes(),
            bytes_antes,
            "corte {corte}: cauda descartada"
        );
        assert_eq!(
            grant_de(&decide(&mut s_cortado, A3, "fs.read", "/p/x")),
            json!(g3),
            "corte {corte}"
        );
        assert_decisao(
            &decide(&mut s_cortado, A1, "fs.read", "/p/x"),
            "allow",
            "ok",
        );
        assert_decisao(
            &decide(&mut s_cortado, A2, "fs.read", "/p/x"),
            "allow",
            "ok",
        );
    }
}

// G27: `delegable:true` no filho sem profundidade restante = invalid_params (6A). A g2 (profundidade 1)
// pode ser delegavel; a g3 (profundidade 2) pedida como delegavel = invalid_params.
// Controle: a mesma g3 sem `delegable` passa e nao delega.
#[test]
fn g27_delegable_do_filho_respeita_a_profundidade_restante() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let g1 = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));
    let g2 = id_de(&delega(
        &mut s,
        A1,
        &g1,
        filho(A2, "fs.read", "/p/x", 1800).com_extra(json!({"delegable": true})),
    ));

    let g3_delegavel = delega(
        &mut s,
        A2,
        &g2,
        filho(A3, "fs.read", "/p/x", 900).com_extra(json!({"delegable": true})),
    );
    assert_eq!(codigo(&g3_delegavel), "invalid_params", "{g3_delegavel}");

    let g3 = id_de(&delega(
        &mut s,
        A2,
        &g2,
        filho(A3, "fs.read", "/p/x", 900).com_extra(json!({"delegable": false})),
    ));
    assert_eq!(grant_de(&decide(&mut s, A3, "fs.read", "/p/x")), json!(g3));
    let neta = delega(&mut s, A3, &g3, filho(A1, "fs.read", "/p/x", 600));
    assert_eq!(codigo(&neta), "exceeds_parent", "g3 nao delegavel: {neta}");
}

// G44: o filho consome e debita o pai (5A.4 e 5A.8f). Pai max_uses 1; tres filhos herdam max_uses 1.
// So um `use` total passa. Controles: antes do uso o check do filho e allow; depois do uso o filho
// vizinho segue negado (o pai esgotou).
#[test]
fn g44_filho_consome_e_debita_o_pai() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let pai = concede_delegavel(
        &mut s,
        A1,
        "fs.read",
        "/p/**",
        3600,
        json!({"constraints": {"max_uses": 1}}),
    );
    for i in 1..=3 {
        let f = delega(
            &mut s,
            A1,
            &pai,
            filho(A2, "fs.read", &format!("/p/{i}"), 600),
        );
        resultado(&f);
    }
    assert_decisao(&decide(&mut s, A2, "fs.read", "/p/1"), "allow", "ok");

    let primeiro = usa(&mut s, A2, "fs.read", "/p/1", &novo_rid(), json!({}));
    assert_eq!(resultado(&primeiro)["replayed"], json!(false), "{primeiro}");

    let vizinho = usa(&mut s, A2, "fs.read", "/p/2", &novo_rid(), json!({}));
    assert_eq!(codigo(&vizinho), "denied", "{vizinho}");
    let terceiro = usa(&mut s, A2, "fs.read", "/p/3", &novo_rid(), json!({}));
    assert_eq!(codigo(&terceiro), "denied", "{terceiro}");
    assert_decisao(&decide(&mut s, A2, "fs.read", "/p/2"), "deny", "exhausted");
}

// G45: deny do delegante sobre o recurso do filho barra a delegacao (5A.8c). O erro nao e nomeado pela
// spec: assumido `forbidden`. Controle: o mesmo delegante delega /p/y, que nao e negado.
#[test]
fn g45_deny_do_delegante_barra_a_delegacao() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let pai = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));
    nega(&mut s, A1, "fs.read", "/p/x", 3600);

    let barrada = delega(&mut s, A1, &pai, filho(A2, "fs.read", "/p/x", 600));
    assert_eq!(codigo(&barrada), "forbidden", "{barrada}");

    let controle = delega(&mut s, A1, &pai, filho(A2, "fs.read", "/p/y", 600));
    resultado(&controle);
    assert_decisao(&decide(&mut s, A2, "fs.read", "/p/y"), "allow", "ok");
}

// G46: recurso do filho com glob = invalid_params; grantee do filho `*` = invalid_params (5A.8b);
// pai com grantee `*` nao e delegavel = exceeds_parent. Controle: o filho concreto com pai do proprio
// delegante passa.
#[test]
fn g46_glob_no_filho_grantee_curinga_e_pai_curinga_nao_delegam() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let pai = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));

    let glob = delega(&mut s, A1, &pai, filho(A2, "fs.read", "/p/**", 600));
    assert_eq!(codigo(&glob), "invalid_params", "{glob}");
    let curinga = delega(&mut s, A1, &pai, filho("*", "fs.read", "/p/x", 600));
    assert_eq!(codigo(&curinga), "invalid_params", "{curinga}");

    let pai_curinga = concede_delegavel(&mut s, "*", "fs.read", "/q/**", 3600, json!({}));
    let de_curinga = delega(&mut s, A1, &pai_curinga, filho(A2, "fs.read", "/q/x", 600));
    assert_eq!(codigo(&de_curinga), "exceeds_parent", "{de_curinga}");

    let controle = delega(&mut s, A1, &pai, filho(A2, "fs.read", "/p/x", 600));
    resultado(&controle);
}

// G47: revogar o pai revoga os filhos ainda `future` (not_before no futuro) no mesmo commit (3 eventos:
// pai, filho active, filho future). `max_children` conta filhos revogados: com 3 filhos e um revogado, o 4o
// e quota. Controle: outro pai com 2 filhos (um revogado) aceita o 3o.
#[test]
fn g47_revoke_alcanca_future_e_max_children_conta_revogados() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let pai = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));
    let futuro = id_de(&pede_como(
        &mut s,
        A1,
        "delegate",
        json!({
            "parent": pai,
            "grantee": A2,
            "action": "fs.read",
            "resource": "/p/1",
            "not_before": em(100),
            "until": em(600),
            "request_id": novo_rid(),
        }),
    ));
    let ativo = id_de(&delega(&mut s, A1, &pai, filho(A2, "fs.read", "/p/2", 600)));

    resultado(&pede_dono(
        &mut s,
        "revoke",
        json!({"id": pai, "reason": "fim", "request_id": novo_rid()}),
    ));
    let tx = ultimo_tx(&d);
    assert_eq!(tx.len(), 3, "pai, filho active e filho future: {tx:?}");
    let ids: Vec<Value> = tx.iter().map(|e| e["id"].clone()).collect();
    for id in [&pai, &futuro, &ativo] {
        assert!(ids.contains(&json!(id)), "faltou {id}: {tx:?}");
    }
    assert_decisao(&decide(&mut s, A2, "fs.read", "/p/2"), "deny", "no_grant");

    // max_children conta revogados.
    let pai2 = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));
    let mut filhos = Vec::new();
    for c in ["/p/a", "/p/b", "/p/c"] {
        filhos.push(id_de(&delega(
            &mut s,
            A1,
            &pai2,
            filho(A2, "fs.read", c, 600),
        )));
    }
    resultado(&pede_dono(
        &mut s,
        "revoke",
        json!({"id": filhos[0], "reason": "saiu", "request_id": novo_rid()}),
    ));
    let quarto = delega(&mut s, A1, &pai2, filho(A2, "fs.read", "/p/d", 600));
    assert_eq!(codigo(&quarto), "quota", "filho revogado conta: {quarto}");

    let pai3 = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));
    for c in ["/p/e", "/p/f"] {
        resultado(&delega(&mut s, A1, &pai3, filho(A2, "fs.read", c, 600)));
    }
    resultado(&delega(
        &mut s,
        A1,
        &pai3,
        filho(A2, "fs.read", "/p/g", 600),
    ));
}

// G48: constraints omitidas no filho herdam as do pai (5A.8e). Pai: max_amount 10 usd, janela 60 s.
// O filho sem constraints nega 11 (exhausted/denied) e aceita 10. Constraints maiores = exceeds_parent;
// `unit` diferente = exceeds_parent (assumido; a spec diz "unit igual" sem codigo). Controle: valores menores passam.
#[test]
fn g48_constraints_omitidas_herdam_as_do_pai() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let pai = concede_delegavel(
        &mut s,
        A1,
        "spend",
        "openai:x",
        3600,
        json!({"constraints": {"max_amount": 10, "unit": "usd", "window_s": 60}}),
    );

    let filho_sem = id_de(&delega(
        &mut s,
        A1,
        &pai,
        filho(A2, "spend", "openai:x", 600),
    ));
    assert_eq!(
        grant_de(&decide_qty(&mut s, A2, "spend", "openai:x", 10)),
        json!(filho_sem)
    );
    assert_decisao(
        &decide_qty(&mut s, A2, "spend", "openai:x", 11),
        "deny",
        "exhausted",
    );
    let negado = usa(
        &mut s,
        A2,
        "spend",
        "openai:x",
        &novo_rid(),
        json!({"qty": 11, "unit": "usd"}),
    );
    assert_eq!(codigo(&negado), "denied", "{negado}");
    let aceito = usa(
        &mut s,
        A2,
        "spend",
        "openai:x",
        &novo_rid(),
        json!({"qty": 10, "unit": "usd"}),
    );
    assert_eq!(resultado(&aceito)["replayed"], json!(false), "{aceito}");

    let maior = delega(
        &mut s,
        A1,
        &pai,
        filho(A2, "spend", "openai:x", 600)
            .com_extra(json!({"constraints": {"max_amount": 20, "unit": "usd", "window_s": 60}})),
    );
    assert_eq!(codigo(&maior), "exceeds_parent", "{maior}");
    let outra_unidade = delega(
        &mut s,
        A1,
        &pai,
        filho(A2, "spend", "openai:x", 600)
            .com_extra(json!({"constraints": {"max_amount": 5, "unit": "eur", "window_s": 60}})),
    );
    assert_eq!(codigo(&outra_unidade), "exceeds_parent", "{outra_unidade}");

    let menor = delega(
        &mut s,
        A1,
        &pai,
        filho(A2, "spend", "openai:x", 600)
            .com_extra(json!({"constraints": {"max_amount": 5, "unit": "usd", "window_s": 60}})),
    );
    resultado(&menor);
}

// G56 (5A.8a e 6A): delegar a partir de concessao NAO delegable (profundidade 0, `delegable:false`) =
// `exceeds_parent`. Controle: a mesma concessao com `delegable:true` delega e o filho decide allow.
#[test]
fn g56_delegar_de_concessao_nao_delegavel_e_exceeds_parent() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let nao = id_de(&pede_dono(
        &mut s,
        "grant",
        com(
            allow(A1, "fs.read", "/p/**"),
            json!({"until": em(3600), "delegable": false, "request_id": novo_rid()}),
        ),
    ));
    let v = delega(&mut s, A1, &nao, filho(A2, "fs.read", "/p/x", 600));
    assert_eq!(codigo(&v), "exceeds_parent", "{v}");

    let sim = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));
    resultado(&delega(&mut s, A1, &sim, filho(A2, "fs.read", "/p/x", 600)));
    assert_decisao(&decide(&mut s, A2, "fs.read", "/p/x"), "allow", "ok");
}

// G57 (5A.8b e 16): a2 (com `can_delegate`) nao delega concessao cujo grantee e a1 = `forbidden`; pai com
// grantee `*` (nao delegavel, 5A.8b) = `exceeds_parent`. Controles: a1 delegando a propria concessao passa;
// e o pedido recusado nao grava.
#[test]
fn g57_delegar_concessao_alheia_e_forbidden_e_pai_curinga_exceeds() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let do_a1 = concede_delegavel(&mut s, A1, "fs.read", "/p/**", 3600, json!({}));
    let h = head(&mut s);

    let alheia = delega(&mut s, A2, &do_a1, filho(A3, "fs.read", "/p/x", 600));
    assert_eq!(codigo(&alheia), "forbidden", "{alheia}");
    assert_eq!(head(&mut s), h, "recusa nao grava");

    let curinga = concede_delegavel(&mut s, "*", "fs.read", "/q/**", 3600, json!({}));
    let de_curinga = delega(&mut s, A1, &curinga, filho(A2, "fs.read", "/q/x", 600));
    assert_eq!(codigo(&de_curinga), "exceeds_parent", "{de_curinga}");

    resultado(&delega(
        &mut s,
        A1,
        &do_a1,
        filho(A2, "fs.read", "/p/x", 600),
    ));
    assert_decisao(&decide(&mut s, A2, "fs.read", "/p/x"), "allow", "ok");
}

// G58 (5A.8e e 16): `window_s` do filho diferente do pai = `exceeds_parent`; filho sem `window_s` sob pai com
// `window_s` diverge (a ausencia e "vida da concessao", nao 60 s). Controle: janela igual (60 s) passa.
#[test]
fn g58_window_s_divergente_no_filho_e_exceeds_parent() {
    let d = Disco::novo();
    let r = Relogio::novo();
    let mut s = sobe(&d, &r, &grants_de(&perfis_delegacao()));
    let pai = concede_delegavel(
        &mut s,
        A1,
        "spend",
        "openai:w",
        3600,
        json!({"constraints": {"max_amount": 10, "unit": "usd", "window_s": 60}}),
    );

    let menor = delega(
        &mut s,
        A1,
        &pai,
        filho(A2, "spend", "openai:w", 600)
            .com_extra(json!({"constraints": {"max_amount": 5, "unit": "usd", "window_s": 30}})),
    );
    assert_eq!(
        codigo(&menor),
        "exceeds_parent",
        "window 30 sob pai 60: {menor}"
    );

    let sem = delega(
        &mut s,
        A1,
        &pai,
        filho(A2, "spend", "openai:w", 600)
            .com_extra(json!({"constraints": {"max_amount": 5, "unit": "usd"}})),
    );
    assert_eq!(
        codigo(&sem),
        "exceeds_parent",
        "window ausente sob pai 60: {sem}"
    );

    let igual = delega(
        &mut s,
        A1,
        &pai,
        filho(A2, "spend", "openai:w", 600)
            .com_extra(json!({"constraints": {"max_amount": 5, "unit": "usd", "window_s": 60}})),
    );
    resultado(&igual);
}
