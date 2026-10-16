//! Escrita (spec secao 5) e validacao de entrada (secao 8, B29-B30).
//! Caixa preta: so a API da secao 8; falhas injetadas pelo fake do `Disco`.
mod common;

use blocklog::{CommitError, Options};
use common::*;
use serde_json::{Value, json};

/// Linha do commit (seq 1, ts TS_123, "um", req (a, r1, hash(1)), res ok) com o \n final.
fn primeira_linha_nl() -> String {
    format!("{}\n", lin(1, TS_123, "um"))
}

fn req_1() -> Option<blocklog::Req> {
    Some(rq("a", "r1", &hash(1)))
}

// ---------------------------------------------------------------- B9

#[test]
fn b09_sucesso_grava_uma_linha_e_um_sync_data() {
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), Options::default());

    let m = disco.marca();
    let c = ok(log.commit(vec![ev("um")], req_1(), Some(ok_json())));
    assert_eq!(c.seq, 1);
    assert!(!c.replayed);

    let esperada = primeira_linha_nl();
    assert_eq!(
        disco.ops_desde(m),
        vec![Op::Write(esperada.len()), Op::SyncData],
        "um write_all com a linha inteira, seguido de um sync_data"
    );
    assert_eq!(disco.bytes(), esperada.into_bytes());
    assert_eq!(log.head_seq(), 1);
    assert!(!log.is_broken());
}

// ---------------------------------------------------------------- B10, B11, B12, B13

#[test]
fn b10_write_all_falho_deixa_broken_sem_aplicar_e_commit_seguinte_broken() {
    // controle: sem falha, o mesmo commit e aceito
    let d0 = Disco::novo();
    let (mut l0, _) = abre_ok(&d0, &RelogioFalso::em(TS_123), Options::default());
    assert_eq!(
        ok(l0.commit(vec![ev("um")], req_1(), Some(ok_json()))).seq,
        1
    );
    assert!(!l0.is_broken());

    let disco = Disco::novo();
    let relogio = RelogioFalso::em(TS_123);
    let (mut log, _) = abre_ok(&disco, &relogio, Options::default());
    disco.arma_escrita(Falha::Eio);

    let m = disco.marca();
    let e = erro(log.commit(vec![ev("um")], req_1(), Some(ok_json())));
    assert!(matches!(e, CommitError::Io(_)), "{e:?}");
    assert_eq!(
        disco.ops_desde(m),
        vec![Op::Write(primeira_linha_nl().len())],
        "uma tentativa de escrita e nenhuma outra"
    );
    assert!(disco.bytes().is_empty(), "nada gravado");
    assert!(log.is_broken());
    assert_eq!(log.head_seq(), 0, "nada aplicado");

    // leitura continua: head_seq segue 0 e o commit seguinte e Broken, sem tocar o disco
    let m2 = disco.marca();
    let e2 = erro(log.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(2))),
        Some(ok_json()),
    ));
    assert!(matches!(e2, CommitError::Broken), "{e2:?}");
    assert!(disco.ops_desde(m2).is_empty());
    assert_eq!(log.head_seq(), 0);

    // reabrir o mesmo disco: nada foi gravado, log limpo e sem broken
    drop(log);
    let (log2, c) = abre_ok(&disco, &relogio, Options::default());
    assert!(c.is_empty());
    assert!(!log2.is_broken());
}

#[test]
fn b11_sync_data_falho_deixa_broken_e_a_linha_pode_existir_apos_reabrir() {
    // controle: sem falha, o commit e aceito
    let d0 = Disco::novo();
    let (mut l0, _) = abre_ok(&d0, &RelogioFalso::em(TS_123), Options::default());
    assert_eq!(
        ok(l0.commit(vec![ev("um")], req_1(), Some(ok_json()))).seq,
        1
    );

    let disco = Disco::novo();
    let relogio = RelogioFalso::em(TS_123);
    let (mut log, _) = abre_ok(&disco, &relogio, Options::default());
    disco.arma_sync(Falha::Eio);

    let m = disco.marca();
    let e = erro(log.commit(vec![ev("um")], req_1(), Some(ok_json())));
    assert!(matches!(e, CommitError::Io(_)), "{e:?}");
    assert_eq!(
        disco.ops_desde(m),
        vec![Op::Write(primeira_linha_nl().len()), Op::SyncData],
        "a escrita ocorreu antes do sync que falhou"
    );
    assert!(log.is_broken());
    assert_eq!(log.head_seq(), 0, "o chamador nao deve aplicar nada");

    let m2 = disco.marca();
    let e2 = erro(log.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(2))),
        Some(ok_json()),
    ));
    assert!(matches!(e2, CommitError::Broken), "{e2:?}");
    assert!(disco.ops_desde(m2).is_empty());

    // G2: a linha pode ou nao existir apos reiniciar. Aceita-se as duas formas, com verificacao exata
    drop(log);
    let (mut log2, c) = abre_ok(&disco, &relogio, Options::default());
    match c.as_slice() {
        [] => assert!(disco.bytes().is_empty(), "sem commit, sem bytes"),
        [unico] => assert_eq!(
            *unico,
            esperado(1, TS_123, "um"),
            "se existe, e exatamente o commit tentado"
        ),
        outro => panic!("no maximo um commit: {outro:?}"),
    }
    assert!(!log2.is_broken());
    let proximo = c.len() as u64 + 1;
    let novo = ok(log2.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(2))),
        Some(ok_json()),
    ));
    assert_eq!(novo.seq, proximo, "seq segue o que de fato esta no log");
}

#[test]
fn b12_escrita_curta_deixa_broken_e_a_cauda_some_na_recuperacao() {
    // controle: sem falha, a linha inteira e gravada
    let d0 = Disco::novo();
    let (mut l0, _) = abre_ok(&d0, &RelogioFalso::em(TS_123), Options::default());
    ok(l0.commit(vec![ev("um")], req_1(), Some(ok_json())));
    assert_eq!(d0.bytes(), primeira_linha_nl().into_bytes());

    let disco = Disco::novo();
    let relogio = RelogioFalso::em(TS_123);
    let (mut log, _) = abre_ok(&disco, &relogio, Options::default());
    disco.arma_escrita(Falha::EscritaCurta(10));

    let e = erro(log.commit(vec![ev("um")], req_1(), Some(ok_json())));
    assert!(matches!(e, CommitError::Io(_)), "{e:?}");
    assert_eq!(
        disco.bytes(),
        primeira_linha_nl().as_bytes()[..10].to_vec(),
        "so o prefixo de 10 bytes ficou no arquivo"
    );
    assert!(log.is_broken());
    assert_eq!(log.head_seq(), 0);
    let e2 = erro(log.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(2))),
        Some(ok_json()),
    ));
    assert!(matches!(e2, CommitError::Broken), "{e2:?}");

    // reabrir: o prefixo sem \n e cauda; e descartado e o arquivo fica vazio
    drop(log);
    let m = disco.marca();
    let (mut log2, c) = abre_ok(&disco, &relogio, Options::default());
    assert!(c.is_empty());
    assert!(disco.bytes().is_empty());
    assert!(
        disco.ops_desde(m).contains(&Op::SetLen(0)),
        "truncado para zero"
    );
    assert!(!log2.is_broken());
    let novo = ok(log2.commit(vec![ev("um")], req_1(), Some(ok_json())));
    assert_eq!(novo.seq, 1);
}

#[test]
fn b13_enospc_deixa_broken_na_escrita_e_na_sincronizacao() {
    // controle: sem falha
    let d0 = Disco::novo();
    let (mut l0, _) = abre_ok(&d0, &RelogioFalso::em(TS_123), Options::default());
    assert_eq!(
        ok(l0.commit(vec![ev("um")], req_1(), Some(ok_json()))).seq,
        1
    );

    // ENOSPC na escrita
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), Options::default());
    disco.arma_escrita(Falha::Enospc);
    match erro(log.commit(vec![ev("um")], req_1(), Some(ok_json()))) {
        CommitError::Io(io) => assert_eq!(io.raw_os_error(), Some(28), "ENOSPC propagado"),
        outro => panic!("esperava Io(ENOSPC), veio {outro:?}"),
    }
    assert!(disco.bytes().is_empty());
    assert!(log.is_broken());
    let e2 = erro(log.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(2))),
        Some(ok_json()),
    ));
    assert!(matches!(e2, CommitError::Broken), "{e2:?}");

    // ENOSPC na sincronizacao
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), Options::default());
    disco.arma_sync(Falha::Enospc);
    match erro(log.commit(vec![ev("um")], req_1(), Some(ok_json()))) {
        CommitError::Io(io) => assert_eq!(io.raw_os_error(), Some(28)),
        outro => panic!("esperava Io(ENOSPC), veio {outro:?}"),
    }
    assert!(log.is_broken());
    assert_eq!(log.head_seq(), 0);
    let e2 = erro(log.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(2))),
        Some(ok_json()),
    ));
    assert!(matches!(e2, CommitError::Broken), "{e2:?}");
}

// ---------------------------------------------------------------- B14, B15

#[test]
fn b14_linha_acima_de_line_max_e_too_large_sem_escrita_e_sem_broken() {
    let opts = Options {
        line_max: 4096,
        ..Options::default()
    };
    // linha com um evento {"k":"<n x's>"}; o tamanho cresce 1 byte por x
    let linha_com = |n: usize| {
        linha(
            1,
            TS_123,
            &format!(r#"{{"k":"{}"}}"#, "x".repeat(n)),
            "a",
            "r1",
            &hash(1),
            OK,
        )
    };
    let limite = 4096 - linha_com(0).len();
    assert_eq!(linha_com(limite).len(), 4096, "calculo do limite exato");

    // controle: exatamente line_max bytes (sem \n) e aceito
    let d0 = Disco::novo();
    let (mut l0, _) = abre_ok(&d0, &RelogioFalso::em(TS_123), opts.clone());
    let c = ok(l0.commit(vec![ev(&"x".repeat(limite))], req_1(), Some(ok_json())));
    assert_eq!(c.seq, 1);
    assert_eq!(d0.bytes(), format!("{}\n", linha_com(limite)).into_bytes());

    // um byte acima: TooLarge, sem escrita e sem broken
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), opts.clone());
    let m = disco.marca();
    let e = erro(log.commit(vec![ev(&"x".repeat(limite + 1))], req_1(), Some(ok_json())));
    assert!(matches!(e, CommitError::TooLarge), "{e:?}");
    assert!(disco.ops_desde(m).is_empty());
    assert!(disco.bytes().is_empty());
    assert!(!log.is_broken());
    assert_eq!(log.head_seq(), 0);

    let c2 = ok(log.commit(vec![ev("um")], req_1(), Some(ok_json())));
    assert_eq!(c2.seq, 1, "o log segue usavel");
}

#[test]
fn b15_tx_acima_de_tx_max_ou_vazio_recusa_sem_gravar() {
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), Options::default());
    let m = disco.marca();

    let dezessete: Vec<Value> = (0..17).map(|_| ev("e")).collect();
    let e = erro(log.commit(dezessete, req_1(), Some(ok_json())));
    assert!(matches!(e, CommitError::TxTooLarge), "{e:?}");

    let e = erro(log.commit(vec![], req_1(), Some(ok_json())));
    assert!(matches!(e, CommitError::EmptyTx), "{e:?}");

    assert!(disco.ops_desde(m).is_empty(), "nada gravado");
    assert!(!log.is_broken());
    assert_eq!(log.head_seq(), 0);

    // controle: 16 eventos (tx_max padrao) e aceito
    let dezesseis: Vec<Value> = (0..16).map(|_| ev("e")).collect();
    let c = ok(log.commit(dezesseis, req_1(), Some(ok_json())));
    assert_eq!(c.seq, 1);
    let tx_txt = vec![r#"{"k":"e"}"#; 16].join(",");
    assert_eq!(
        disco.bytes(),
        format!("{}\n", linha(1, TS_123, &tx_txt, "a", "r1", &hash(1), OK)).into_bytes()
    );
}

// ---------------------------------------------------------------- B16, B17

#[test]
fn b16_log_max_bytes_recusa_com_log_full_sem_broken() {
    const MAX: u64 = 1_048_576; // minimo da faixa de log_max_bytes
    let opts = Options {
        line_max: 4_194_304,
        log_max_bytes: MAX,
        ..Options::default()
    };
    // commit de seq 2 que sera feito no teste (ts = TS_123, conforme o relogio)
    let l2 = lin(2, TS_123, "dois");
    // Arquivo: len(l1) + 1 + len(l2) + 1 == MAX. Logo len(l1) (sem \n) = MAX - len(l2) - 2.
    let alvo = MAX as usize - l2.len() - 2;
    let l1_com = |n: usize| lin(1, TS_100, &"x".repeat(n));
    let base = l1_com(0).len();
    let l1 = l1_com(alvo - base);
    assert_eq!(l1.len(), alvo, "calculo do tamanho da linha 1");

    let disco = Disco::com(&arq(&[l1.clone()]));
    let (mut log, c) = abre_ok(&disco, &RelogioFalso::em(TS_123), opts);
    assert_eq!(c.len(), 1);

    // controle: chegar exatamente em log_max_bytes e aceito (ultrapassar e so passar do limite)
    let c2 = ok(log.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(2))),
        Some(ok_json()),
    ));
    assert_eq!(c2.seq, 2);
    assert_eq!(disco.bytes(), arq(&[l1.clone(), l2.clone()]));
    assert_eq!(disco.bytes().len() as u64, MAX);

    // qualquer commit seguinte ultrapassa: LogFull, sem escrita e sem broken
    let m = disco.marca();
    let e = erro(log.commit(
        vec![ev("x")],
        Some(rq("a", "r3", &hash(3))),
        Some(ok_json()),
    ));
    assert!(matches!(e, CommitError::LogFull), "{e:?}");
    assert!(disco.ops_desde(m).is_empty());
    assert!(!log.is_broken());
    assert_eq!(log.head_seq(), 2);
    let e2 = erro(log.commit(vec![ev("x")], None, None));
    assert!(matches!(e2, CommitError::LogFull), "{e2:?}");
}

#[test]
fn b17_ts_nao_regride_com_relogio_regressivo() {
    let disco = Disco::novo();
    let relogio = RelogioFalso::em(TS_500);
    let (mut log, _) = abre_ok(&disco, &relogio, Options::default());

    ok(log.commit(vec![ev("um")], req_1(), Some(ok_json())));
    relogio.poe(TS_100); // relogio voltou 400 ms
    ok(log.commit(
        vec![ev("dois")],
        Some(rq("a", "r2", &hash(2))),
        Some(ok_json()),
    ));
    assert_eq!(
        disco.bytes(),
        arq(&[lin(1, TS_500, "um"), lin(2, TS_500, "dois")]),
        "ts da segunda linha = max(agora, ultimo) = TS_500"
    );

    relogio.poe(TS_900); // controle: quando o relogio anda, o ts acompanha
    let c3 = ok(log.commit(
        vec![ev("tres")],
        Some(rq("a", "r3", &hash(3))),
        Some(ok_json()),
    ));
    assert_eq!(c3.seq, 3);
    assert_eq!(
        disco.bytes(),
        arq(&[
            lin(1, TS_500, "um"),
            lin(2, TS_500, "dois"),
            lin(3, TS_900, "tres"),
        ])
    );

    // a reabertura aceita a sequencia (ts nao regridem no arquivo)
    drop(log);
    let (_l, c) = abre_ok(
        &disco,
        &RelogioFalso::em("2026-10-16T23:00:00.000Z"),
        Options::default(),
    );
    assert_eq!(
        c.iter().map(|x| x.ts.as_str()).collect::<Vec<_>>(),
        vec![TS_500, TS_500, TS_900]
    );
}

// ---------------------------------------------------------------- B29, B30 (entrada invalida)

#[test]
fn b29_elemento_de_tx_nao_objeto_e_invalid_input_sem_gravar_nem_broken() {
    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), Options::default());

    let invalidos = vec![
        json!("texto"),
        json!(3),
        json!(null),
        json!([1, 2]),
        json!(true),
    ];
    for (i, bad) in invalidos.into_iter().enumerate() {
        let m = disco.marca();
        let e = erro(log.commit(vec![bad.clone()], req_1(), Some(ok_json())));
        assert!(
            matches!(e, CommitError::InvalidInput(_)),
            "caso {i} ({bad}): {e:?}"
        );
        assert!(disco.ops_desde(m).is_empty(), "caso {i}: nada gravado");
        assert!(!log.is_broken(), "caso {i}: sem broken");
    }

    // tx misto: um objeto e um nao-objeto
    let m = disco.marca();
    let e = erro(log.commit(vec![ev("um"), json!(3)], req_1(), Some(ok_json())));
    assert!(matches!(e, CommitError::InvalidInput(_)), "{e:?}");
    assert!(disco.ops_desde(m).is_empty());

    // controle: so objetos, com o mesmo req. Se as recusas tivessem indexado algo, daria replay
    let c = ok(log.commit(vec![ev("um")], req_1(), Some(ok_json())));
    assert_eq!(c.seq, 1);
    assert!(!c.replayed);
    assert_eq!(disco.bytes(), arq(&[lin(1, TS_123, "um")]));
}

#[test]
fn b30_req_fora_do_formato_e_invalid_input_e_limites_validos_passam() {
    let casos: Vec<(&str, blocklog::Req)> = vec![
        ("agent com maiuscula", rq("Alice", "r1", &hash(1))),
        (
            "agent de 33 caracteres",
            rq(&"a".repeat(33), "r1", &hash(1)),
        ),
        ("agent vazio", rq("", "r1", &hash(1))),
        ("id com espaco", rq("a", "r 1", &hash(1))),
        ("id de 65 caracteres", rq("a", &"i".repeat(65), &hash(1))),
        ("h de 63 caracteres", rq("a", "r1", &"a".repeat(63))),
        ("h de 65 caracteres", rq("a", "r1", &"a".repeat(65))),
        ("h com maiuscula", rq("a", "r1", &"A".repeat(64))),
        (
            "h com caractere nao hexadecimal",
            rq("a", "r1", &"g".repeat(64)),
        ),
    ];

    let disco = Disco::novo();
    let (mut log, _) = abre_ok(&disco, &RelogioFalso::em(TS_123), Options::default());
    for (nome, req) in casos {
        let m = disco.marca();
        let e = erro(log.commit(vec![ev("um")], Some(req), Some(ok_json())));
        assert!(matches!(e, CommitError::InvalidInput(_)), "{nome}: {e:?}");
        assert!(disco.ops_desde(m).is_empty(), "{nome}: nada gravado");
        assert!(!log.is_broken(), "{nome}: sem broken");
    }

    // controle: o mesmo req bem formado passa com seq 1 (as recusas nao gastaram seq nem indice)
    let c = ok(log.commit(vec![ev("um")], req_1(), Some(ok_json())));
    assert_eq!(c.seq, 1);

    // controle nos limites validos: agent de 32, id de 64 caracteres
    let disco2 = Disco::novo();
    let (mut log2, _) = abre_ok(&disco2, &RelogioFalso::em(TS_123), Options::default());
    let limite = ok(log2.commit(
        vec![ev("um")],
        Some(rq(&"a".repeat(32), &"i".repeat(64), &hash(1))),
        Some(ok_json()),
    ));
    assert_eq!(limite.seq, 1, "limites da faixa sao validos");
}
