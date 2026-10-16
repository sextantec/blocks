//! Idempotencia por (agent, id) com h (spec secao 5 passo 2, secao 6; B18-B22).
//! Caixa preta: so a API da secao 8.
mod common;

use blocklog::{CommitError, Committed, Options};
use common::*;
use serde_json::json;

#[test]
fn b18_mesma_agent_id_h_devolve_res_original_com_replayed_e_nao_grava() {
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), Options::default());

    let c1 = ok(log.commit(
        vec![ev("um")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": true})),
    ));
    assert_eq!((c1.seq, c1.replayed), (1, false));

    let antes = disco.bytes();
    let m = disco.marca();
    // mesmo (agent, id, h); tx e res diferentes: o res recebido e ignorado
    let c2 = ok(log.commit(
        vec![ev("outra carga")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": "novo"})),
    ));
    assert_eq!(
        c2,
        Committed {
            seq: 1,
            replayed: true,
            res: Some(json!({"ok": true})),
        }
    );
    assert!(disco.ops_desde(m).is_empty(), "replay nao grava");
    assert_eq!(disco.bytes(), antes);
    assert_eq!(log.head_seq(), 1);

    // controle: outro id segue a sequencia
    let c3 = ok(log.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(1))),
        Some(json!({"ok": 2})),
    ));
    assert_eq!(c3.seq, 2);
    assert!(!c3.replayed);
}

#[test]
fn b19_mesmo_id_com_h_diferente_e_request_conflict_sem_gravar() {
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), Options::default());
    let c1 = ok(log.commit(
        vec![ev("um")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": true})),
    ));
    assert_eq!(c1.seq, 1);

    let m = disco.marca();
    let e = erro(log.commit(
        vec![ev("outra")],
        Some(rq("a", "r1", &hash(2))),
        Some(json!({"ok": true})),
    ));
    assert!(matches!(e, CommitError::RequestConflict), "{e:?}");
    assert!(disco.ops_desde(m).is_empty(), "conflito nao grava");
    assert!(!log.is_broken());
    assert_eq!(log.head_seq(), 1);

    // controle: o h original continua valido e devolve replay
    let c2 = ok(log.commit(
        vec![ev("um")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": "x"})),
    ));
    assert_eq!(
        c2,
        Committed {
            seq: 1,
            replayed: true,
            res: Some(json!({"ok": true})),
        }
    );

    // controle: outro agente com o mesmo id e outra chave (agent, id): commit novo
    let c3 = ok(log.commit(
        vec![ev("outro agente")],
        Some(rq("b", "r1", &hash(2))),
        Some(json!({"ok": 3})),
    ));
    assert_eq!(c3.seq, 2);
    assert!(!c3.replayed);
}

#[test]
fn b20_idempotencia_sobrevive_a_reabertura() {
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), Options::default());
    let c1 = ok(log.commit(
        vec![ev("um")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": true})),
    ));
    assert_eq!(c1.seq, 1);
    drop(log);

    // reabre logo depois (dentro da retencao)
    let (mut log2, c) = abre_ok(&disco, &RelogioFalso::em(TS_ABERTURA), Options::default());
    assert_eq!(c.len(), 1);
    let m = disco.marca();
    let r = ok(log2.commit(
        vec![ev("outra")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": "nao vale"})),
    ));
    assert_eq!(
        r,
        Committed {
            seq: 1,
            replayed: true,
            res: Some(json!({"ok": true})),
        },
        "o res original, gravado no arquivo, volta depois de reabrir"
    );
    assert!(
        disco.ops_desde(m).is_empty(),
        "replay depois de reabrir nao grava"
    );

    // controle: outro id depois de reabrir recebe seq 2
    let c2 = ok(log2.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(1))),
        Some(json!({"ok": 2})),
    ));
    assert_eq!(c2.seq, 2);
    assert!(!c2.replayed);
}

#[test]
fn b21_fora_da_retencao_o_mesmo_agent_id_conta_como_pedido_novo() {
    let opts = Options {
        request_retention_days: 1,
        ..Options::default()
    };

    // primeiro commit em 16/10 22:00:00.123Z
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), opts.clone());
    let c1 = ok(log.commit(
        vec![ev("um")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": "R1"})),
    ));
    assert_eq!(c1.seq, 1);
    drop(log);

    // reabre 2 dias depois: a entrada venceu, o mesmo (agent, id, h) e pedido novo
    let (mut log2, _) = abre_ok(
        &disco,
        &RelogioFalso::em("2026-10-18T22:00:00.123Z"),
        opts.clone(),
    );
    let c2 = ok(log2.commit(
        vec![ev("um")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": "R2"})),
    ));
    assert_eq!(
        (c2.seq, c2.replayed),
        (2, false),
        "fora da retencao: seq novo, sem replay"
    );

    // controle, em outro disco: reabre 12 h depois, ainda dentro da retencao, e ha replay
    let disco_ctl = Disco::novo();
    let (mut lc, _) = abre_ok(&disco_ctl, &RelogioFalso::em(TS_123), opts.clone());
    ok(lc.commit(
        vec![ev("um")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": "R1"})),
    ));
    drop(lc);
    let (mut lc2, _) = abre_ok(
        &disco_ctl,
        &RelogioFalso::em("2026-10-17T10:00:00.123Z"),
        opts,
    );
    let r = ok(lc2.commit(
        vec![ev("um")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": "R2"})),
    ));
    assert_eq!(
        r,
        Committed {
            seq: 1,
            replayed: true,
            res: Some(json!({"ok": "R1"})),
        },
        "dentro da retencao: replay do res original"
    );
}

#[test]
fn b22_req_sem_res_e_recusado_com_missing_res() {
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), Options::default());

    let m = disco.marca();
    let e = erro(log.commit(vec![ev("um")], Some(rq("a", "r1", &hash(1))), None));
    assert!(matches!(e, CommitError::MissingRes), "{e:?}");
    assert!(disco.ops_desde(m).is_empty(), "nada gravado");
    assert!(!log.is_broken());
    assert_eq!(log.head_seq(), 0);

    // controle: o mesmo req com res e aceito com seq 1
    let c = ok(log.commit(
        vec![ev("um")],
        Some(rq("a", "r1", &hash(1))),
        Some(json!({"ok": true})),
    ));
    assert_eq!(c.seq, 1);
}
