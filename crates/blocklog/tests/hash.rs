//! SHA-256 (spec secao 8, `sha256_hex`; B28) contra vetores NIST (FIPS 180-2 / 180-4).
use blocklog::sha256_hex;

#[test]
fn b28_sha256_contra_vetores_nist() {
    let milhao = vec![b'a'; 1_000_000];
    let vetores: Vec<(&[u8], &str)> = vec![
        // mensagem vazia
        (
            "".as_bytes(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            "abc".as_bytes(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        // 448 bits
        (
            "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".as_bytes(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        // 896 bits (vetor de dois blocos adicional, FIPS 180-2)
        (
            "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
                .as_bytes(),
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
        ),
        // um milhao de 'a'
        (
            milhao.as_slice(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
        ),
    ];

    for (entrada, esperado) in vetores {
        let obtido = sha256_hex(entrada);
        assert_eq!(obtido, esperado, "entrada de {} bytes", entrada.len());
        assert_eq!(obtido.len(), 64, "64 caracteres");
        assert!(
            obtido
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "hexadecimal minusculo: {obtido}"
        );
    }
}
