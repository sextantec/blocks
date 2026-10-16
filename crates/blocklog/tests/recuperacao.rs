//! Abertura e recuperacao (spec secao 4): cauda, validacao de linhas, replay de duplicatas
//! (secao 4 item 4, B31-B32) e reconstrucao (B27). Caixa preta: so a API da secao 8.
mod common;

use blocklog::{Commit, CommitError, Options};
use common::*;
use serde_json::json;

// ---------------------------------------------------------------- cauda (B1, B2, B8)

#[test]
fn b01_cauda_sem_newline_com_json_parcial_e_descartada() {
    let l1 = lin(1, TS_100, "um");
    let l2 = lin(2, TS_200, "dois");
    let mut bytes = arq(&[l1.clone()]);
    bytes.extend_from_slice(&l2.as_bytes()[..40]); // JSON incompleto, sem \n
    let disco = Disco::com(&bytes);

    let (log, commits) = abre_ok(&disco, &RelogioFalso::em(TS_ABERTURA), Options::default());

    assert_eq!(commits, vec![esperado(1, TS_100, "um")]);
    assert_eq!(
        disco.bytes(),
        arq(&[l1.clone()]),
        "sobra so a linha 1 com \\n"
    );
    let ops = disco.ops();
    let tam = (l1.len() + 1) as u64;
    let i_set = ops
        .iter()
        .position(|o| *o == Op::SetLen(tam))
        .expect("truncamento ate o ultimo \\n");
    let i_sync = ops
        .iter()
        .position(|o| *o == Op::SyncAll)
        .expect("sync_all do arquivo");
    assert!(i_set < i_sync, "truncar antes do sync_all: {ops:?}");
    assert!(
        !ops.iter().any(|o| matches!(o, Op::Write(_))),
        "a recuperacao nao escreve: {ops:?}"
    );
    assert_eq!(log.head_seq(), 1);
    assert!(!log.is_broken());
}

#[test]
fn b02_cauda_sem_newline_mesmo_com_json_valido_e_descartada() {
    let l1 = lin(1, TS_100, "um");
    let l2 = lin(2, TS_200, "dois");

    // controle: a mesma linha 2 com \n e um commit valido
    let ctl = Disco::com(&arq(&[l1.clone(), l2.clone()]));
    let (_c, commits_ctl) = abre_ok(&ctl, &RelogioFalso::em(TS_ABERTURA), Options::default());
    assert_eq!(commits_ctl.len(), 2, "controle: linha 2 com \\n vale");

    // caso: linha 2 e JSON valido, mas sem \n
    let mut bytes = arq(&[l1.clone()]);
    bytes.extend_from_slice(l2.as_bytes());
    let disco = Disco::com(&bytes);
    let (log, commits) = abre_ok(&disco, &RelogioFalso::em(TS_ABERTURA), Options::default());

    assert_eq!(commits, vec![esperado(1, TS_100, "um")]);
    assert_eq!(disco.bytes(), arq(&[l1.clone()]));
    assert_eq!(log.head_seq(), 1);
}

#[test]
fn b08_apos_recuperacao_arquivo_termina_em_newline_e_novo_commit_tem_seq_certo() {
    let l1 = lin(1, TS_100, "um");
    let mut bytes = arq(&[l1.clone()]);
    bytes.extend_from_slice(&lin(2, TS_200, "dois").as_bytes()[..40]);
    let disco = Disco::com(&bytes);

    let (mut log, c) = abre_ok(&disco, &RelogioFalso::em(TS_ABERTURA), Options::default());
    assert_eq!(c.len(), 1);
    let novo = ok(log.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(2))),
        Some(ok_json()),
    ));
    assert_eq!(novo.seq, 2);
    assert!(!novo.replayed);

    let depois = disco.bytes();
    assert_eq!(depois.last(), Some(&b'\n'), "termina em \\n");
    assert_eq!(depois, arq(&[l1.clone(), lin(2, TS_ABERTURA, "dois")]));

    drop(log);
    let (log2, c2) = abre_ok(&disco, &RelogioFalso::em(TS_ABERTURA), Options::default());
    assert_eq!(
        c2.iter().map(|x| x.seq).collect::<Vec<_>>(),
        vec![1, 2],
        "reabrir le as duas linhas"
    );
    assert_eq!(log2.head_seq(), 2);
}

// ---------------------------------------------------------------- linhas completas invalidas (B3, B4)

#[test]
fn b03_linha_completa_invalida_na_ultima_posicao_recusa_a_particao() {
    let l1 = lin(1, TS_100, "um");
    let l2 = lin(2, TS_200, "dois");
    let lixo = r#"{"v":1,"seq":2,"ts":"2026-10-16T22:00:00.200Z""#; // JSON invalido, com \n

    // controle: a linha 2 valida na mesma posicao abre com 2 commits
    let ctl = Disco::com(&arq(&[l1.clone(), l2.clone()]));
    assert_eq!(
        abre_ok(&ctl, &RelogioFalso::em(TS_ABERTURA), Options::default())
            .1
            .len(),
        2
    );

    let bytes = arq(&[l1.clone(), lixo.to_string()]);
    let disco = Disco::com(&bytes);
    let e = erro_abre(&disco, &RelogioFalso::em(TS_ABERTURA), Options::default());
    assert_eq!(linha_corrupta(e), 2, "corrupt na linha 2 (1-based)");
    assert_eq!(disco.bytes(), bytes, "linha completa nunca e apagada");
    assert!(!disco.ops().iter().any(|o| matches!(o, Op::SetLen(_))));
}

#[test]
fn b04_linha_invalida_no_meio_recusa_a_particao() {
    let l1 = lin(1, TS_100, "um");
    let l2 = lin(2, TS_200, "dois");
    let l3 = lin(3, TS_300, "tres");

    // controle: as tres linhas validas abrem com 3 commits
    let ctl = Disco::com(&arq(&[l1.clone(), l2.clone(), l3.clone()]));
    assert_eq!(
        abre_ok(&ctl, &RelogioFalso::em(TS_ABERTURA), Options::default())
            .1
            .len(),
        3
    );

    let bytes = arq(&[l1.clone(), "isto nao e json".to_string(), l3.clone()]);
    let disco = Disco::com(&bytes);
    let e = erro_abre(&disco, &RelogioFalso::em(TS_ABERTURA), Options::default());
    assert_eq!(linha_corrupta(e), 2);
    assert_eq!(disco.bytes(), bytes);
}

// ---------------------------------------------------------------- seq e ts (B5, B6)

#[test]
fn b05_seq_com_lacuna_repetido_retrocedido_ou_fora_de_1_recusa() {
    let l1 = lin(1, TS_100, "um");
    let l2 = lin(2, TS_200, "dois");
    let l3 = lin(3, TS_300, "tres");
    // mesma seq 1, mas outro req (a,rX): so a regra de seq e violada
    let seq1_de_novo = linha(1, TS_300, EV_DOIS, "a", "rX", &hash(77), OK);

    // controle
    let ctl = Disco::com(&arq(&[l1.clone(), l2.clone()]));
    assert_eq!(
        abre_ok(&ctl, &RelogioFalso::em(TS_ABERTURA), Options::default())
            .1
            .len(),
        2
    );

    // lacuna: 1, 3
    let d = Disco::com(&arq(&[l1.clone(), l3.clone()]));
    assert_eq!(
        linha_corrupta(erro_abre(
            &d,
            &RelogioFalso::em(TS_ABERTURA),
            Options::default()
        )),
        2,
        "lacuna"
    );

    // repetido: 1, 1
    let d = Disco::com(&arq(&[
        l1.clone(),
        linha(1, TS_200, EV_DOIS, "a", "rX", &hash(77), OK),
    ]));
    assert_eq!(
        linha_corrupta(erro_abre(
            &d,
            &RelogioFalso::em(TS_ABERTURA),
            Options::default()
        )),
        2,
        "repetido"
    );

    // retrocedido: 1, 2, 1
    let d = Disco::com(&arq(&[l1.clone(), l2.clone(), seq1_de_novo]));
    assert_eq!(
        linha_corrupta(erro_abre(
            &d,
            &RelogioFalso::em(TS_ABERTURA),
            Options::default()
        )),
        3,
        "retrocedido"
    );

    // nao comeca em 1: so a linha 2
    let d = Disco::com(&arq(&[l2.clone()]));
    assert_eq!(
        linha_corrupta(erro_abre(
            &d,
            &RelogioFalso::em(TS_ABERTURA),
            Options::default()
        )),
        1,
        "primeiro commit com seq 2"
    );
}

#[test]
fn b06_ts_regressivo_recusa_e_ts_igual_e_aceito() {
    let l1 = lin(1, TS_200, "um");

    // controle: ts igual ao anterior (>=) e valido
    let ctl = Disco::com(&arq(&[l1.clone(), lin(2, TS_200, "dois")]));
    assert_eq!(
        abre_ok(&ctl, &RelogioFalso::em(TS_ABERTURA), Options::default())
            .1
            .len(),
        2
    );

    // caso: ts 100 ms antes do anterior
    let d = Disco::com(&arq(&[l1.clone(), lin(2, TS_100, "dois")]));
    assert_eq!(
        linha_corrupta(erro_abre(
            &d,
            &RelogioFalso::em(TS_ABERTURA),
            Options::default()
        )),
        2
    );
}

// ---------------------------------------------------------------- formato (B7)

#[test]
fn b07_chave_desconhecida_ou_tx_vazio_recusa() {
    let base = lin(1, TS_100, "um");

    // controle
    let ctl = Disco::com(&arq(&[base.clone()]));
    assert_eq!(
        abre_ok(&ctl, &RelogioFalso::em(TS_ABERTURA), Options::default())
            .1
            .len(),
        1
    );

    let casos: Vec<(&str, String)> = vec![
        ("chave extra no envelope", com_chave_extra(&base)),
        (
            "chave extra dentro de req",
            base.replacen(
                &format!("\"h\":\"{}\"", hash(1)),
                &format!("\"h\":\"{}\",\"z\":1", hash(1)),
                1,
            ),
        ),
        (
            "req sem h",
            format!(
                r#"{{"v":1,"seq":1,"ts":"{}","tx":[{{"k":"um"}}],"req":{{"agent":"a","id":"r1"}},"res":{}}}"#,
                TS_100, OK
            ),
        ),
        ("tx vazio", linha(1, TS_100, "", "a", "r1", &hash(1), OK)),
        ("v diferente de 1", base.replacen("\"v\":1", "\"v\":2", 1)),
    ];
    for (nome, l) in casos {
        let d = Disco::com(&arq(&[l]));
        assert_eq!(
            linha_corrupta(erro_abre(
                &d,
                &RelogioFalso::em(TS_ABERTURA),
                Options::default()
            )),
            1,
            "caso: {nome}"
        );
    }
}

// ---------------------------------------------------------------- duplicatas no replay (B31, B32)

#[test]
fn b31_duas_ocorrencias_do_mesmo_agent_id_dentro_da_retencao_recusam_a_partida() {
    let ts_1h_depois = "2026-10-16T23:00:00.000Z";
    // 30 dias menos 1 ms depois de TS_000 (dentro de request_retention_days = 30)
    let ts_quase_30d = "2026-11-15T21:59:59.999Z";

    // controle: mesmo par (agent, id) com agente diferente nao e duplicata
    let ctl = Disco::com(&arq(&[
        linha(1, TS_000, EV_UM, "a", "r1", &hash(1), OK),
        linha(2, TS_000, EV_DOIS, "b", "r1", &hash(1), OK),
    ]));
    assert_eq!(
        abre_ok(&ctl, &RelogioFalso::em(TS_ABERTURA), Options::default())
            .1
            .len(),
        2
    );

    // mesmo h, 1 hora depois
    let bytes = arq(&[
        linha(1, TS_000, EV_UM, "a", "r1", &hash(1), OK),
        linha(2, ts_1h_depois, EV_DOIS, "a", "r1", &hash(1), OK),
    ]);
    let d = Disco::com(&bytes);
    assert_eq!(
        linha_corrupta(erro_abre(
            &d,
            &RelogioFalso::em(TS_ABERTURA),
            Options::default()
        )),
        2,
        "h igual, dentro da retencao"
    );
    assert_eq!(d.bytes(), bytes);

    // h diferente, quase 30 dias depois
    let bytes = arq(&[
        linha(1, TS_000, EV_UM, "a", "r1", &hash(1), OK),
        linha(2, ts_quase_30d, EV_DOIS, "a", "r1", &hash(2), OK),
    ]);
    let d = Disco::com(&bytes);
    assert_eq!(
        linha_corrupta(erro_abre(
            &d,
            &RelogioFalso::em(TS_ABERTURA),
            Options::default()
        )),
        2,
        "h diferente, dentro da retencao"
    );
    assert_eq!(d.bytes(), bytes);
}

#[test]
fn b32_duas_ocorrencias_separadas_por_mais_que_a_retencao_sao_validas_e_a_ultima_vale() {
    // 30 dias e 1 s depois de TS_000: passou da retencao, a segunda e a vigente
    let ts_mais_30d = "2026-11-15T22:00:01.000Z";
    let res2 = r#"{"ok":"segunda"}"#;
    let disco = Disco::com(&arq(&[
        linha(1, TS_000, EV_UM, "a", "r1", &hash(1), OK),
        linha(2, ts_mais_30d, EV_DOIS, "a", "r1", &hash(9), res2),
    ]));

    let (mut log, c) = abre_ok(
        &disco,
        &RelogioFalso::em("2026-11-16T22:00:01.000Z"),
        Options::default(),
    );
    assert_eq!(
        c.iter().map(|x| x.seq).collect::<Vec<_>>(),
        vec![1, 2],
        "as duas ocorrencias ficam no log"
    );

    // a vigente e a segunda (h = hash(9)): repetir com o h dela devolve o res dela
    let m = disco.marca();
    let r = ok(log.commit(
        vec![ev("x")],
        Some(rq("a", "r1", &hash(9))),
        Some(ok_json()),
    ));
    assert_eq!(r.seq, 2);
    assert!(r.replayed);
    assert_eq!(r.res, Some(json!({"ok": "segunda"})));
    assert!(disco.ops_desde(m).is_empty(), "replay nao grava");

    // o h da primeira ocorrencia ja nao e a vigente: conflito
    let e = erro(log.commit(
        vec![ev("x")],
        Some(rq("a", "r1", &hash(1))),
        Some(ok_json()),
    ));
    assert!(matches!(e, CommitError::RequestConflict), "{e:?}");

    // controle: id novo segue a sequencia
    let novo = ok(log.commit(
        vec![ev("y")],
        Some(rq("a", "r3", &hash(3))),
        Some(ok_json()),
    ));
    assert_eq!(novo.seq, 3);
    assert!(!novo.replayed);
}

// ---------------------------------------------------------------- reconstrucao (B27)

#[test]
fn b27_reabrir_devolve_a_mesma_sequencia_que_foi_gravada() {
    let disco = Disco::novo();
    let relogio = RelogioFalso::em(TS_100);
    let (mut log, _) = abre_ok(&disco, &relogio, Options::default());

    let c1 = ok(log.commit(
        vec![ev("um")],
        Some(rq("a", "r1", &hash(1))),
        Some(ok_json()),
    ));
    assert_eq!(c1.seq, 1);
    relogio.poe(TS_200);
    let c2 = ok(log.commit(vec![ev("dois"), ev("tres")], None, None));
    assert_eq!(c2.seq, 2);
    relogio.poe(TS_300);
    let c3 = ok(log.commit(
        vec![json!({"n": 4})],
        Some(rq("b", "r9", &hash(9))),
        Some(json!({"n": 4})),
    ));
    assert_eq!(c3.seq, 3);
    let gravado = disco.bytes();
    drop(log);

    let (log2, commits) = abre_ok(
        &disco,
        &RelogioFalso::em("2026-10-16T23:00:00.000Z"),
        Options::default(),
    );
    assert_eq!(
        commits,
        vec![
            Commit {
                seq: 1,
                ts: TS_100.to_string(),
                tx: vec![ev("um")],
                req: Some(rq("a", "r1", &hash(1))),
                res: Some(ok_json()),
            },
            Commit {
                seq: 2,
                ts: TS_200.to_string(),
                tx: vec![ev("dois"), ev("tres")],
                req: None,
                res: None,
            },
            Commit {
                seq: 3,
                ts: TS_300.to_string(),
                tx: vec![json!({"n": 4})],
                req: Some(rq("b", "r9", &hash(9))),
                res: Some(json!({"n": 4})),
            },
        ]
    );
    assert_eq!(log2.head_seq(), 3);
    assert_eq!(disco.bytes(), gravado, "reabrir sem cauda nao escreve");
}

// ---------------------------------------------------------------- res obrigatorio com req (B33)

#[test]
fn b33_linha_completa_com_req_e_sem_res_recusa_a_particao() {
    let h1 = hash(1);
    let com_res = linha(1, TS_100, EV_UM, "a", "r1", &h1, OK);
    // mesma linha, com \n, sem a chave res
    let sem_res = format!(
        r#"{{"v":1,"seq":1,"ts":"{}","tx":[{}],"req":{{"agent":"a","id":"r1","h":"{}"}}}}"#,
        TS_100, EV_UM, h1
    );

    // controle: a linha com res abre com 1 commit
    let ctl = Disco::com(&arq(&[com_res.clone()]));
    assert_eq!(
        abre_ok(&ctl, &RelogioFalso::em(TS_ABERTURA), Options::default())
            .1
            .len(),
        1
    );

    // sem res, na primeira posicao: Corrupt na linha 1, arquivo intacto
    let bytes = arq(&[sem_res.clone()]);
    let d = Disco::com(&bytes);
    assert_eq!(
        linha_corrupta(erro_abre(
            &d,
            &RelogioFalso::em(TS_ABERTURA),
            Options::default()
        )),
        1
    );
    assert_eq!(d.bytes(), bytes, "linha completa nunca e apagada");
    assert!(!d.ops().iter().any(|o| matches!(o, Op::SetLen(_))));

    // sem res, depois de uma linha valida: Corrupt na linha 2
    let bytes = arq(&[com_res.clone(), sem_res.clone()]);
    let d = Disco::com(&bytes);
    assert_eq!(
        linha_corrupta(erro_abre(
            &d,
            &RelogioFalso::em(TS_ABERTURA),
            Options::default()
        )),
        2
    );
    assert_eq!(d.bytes(), bytes);
}

#[test]
fn b33b_res_explicito_null_com_req_e_json_valido_e_aceito() {
    // Leitura literal (spec 3: "res opcional (JSON qualquer ...)"): null e um valor JSON presente,
    // entao a linha abre. A forma em memoria (None ou Some(null)) nao e fixada pela spec.
    let h1 = hash(1);
    let com_null = linha(1, TS_100, EV_UM, "a", "r1", &h1, "null");
    let d = Disco::com(&arq(&[com_null.clone()]));
    let (log, c) = abre_ok(&d, &RelogioFalso::em(TS_ABERTURA), Options::default());
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].seq, 1);
    assert_eq!(c[0].req, Some(rq("a", "r1", &h1)));
    assert!(
        matches!(c[0].res, None | Some(serde_json::Value::Null)),
        "res null: {:?}",
        c[0].res
    );
    assert_eq!(log.head_seq(), 1);
    assert_eq!(d.bytes(), arq(&[com_null]), "abrir nao escreve");
}

// ---------------------------------------------------------------- line_max na abertura (B34)

#[test]
fn b34_linha_completa_valida_maior_que_line_max_recusa_a_abertura() {
    let grande = linha(
        1,
        TS_100,
        &format!(r#"{{"k":"{}"}}"#, "x".repeat(5000)),
        "a",
        "r1",
        &hash(1),
        OK,
    );
    assert!(grande.len() > 4096, "linha de teste maior que 4096");
    let bytes = arq(&[grande.clone()]);

    // controle: com line_max maior (padrao), a mesma linha abre
    let d0 = Disco::com(&bytes);
    assert_eq!(
        abre_ok(&d0, &RelogioFalso::em(TS_ABERTURA), Options::default())
            .1
            .len(),
        1
    );

    // line_max = 4096: a linha de ~5000 bytes e recusada na abertura, arquivo intacto
    let pequeno = Options {
        line_max: 4096,
        ..Options::default()
    };
    let d = Disco::com(&bytes);
    assert_eq!(
        linha_corrupta(erro_abre(
            &d,
            &RelogioFalso::em(TS_ABERTURA),
            pequeno.clone()
        )),
        1
    );
    assert_eq!(d.bytes(), bytes);

    // controle no mesmo line_max: uma linha curta valida abre
    let curta = Disco::com(&arq(&[lin(1, TS_100, "um")]));
    assert_eq!(
        abre_ok(&curta, &RelogioFalso::em(TS_ABERTURA), pequeno)
            .1
            .len(),
        1
    );
}
