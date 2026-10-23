// Independent review probes — run against the built lib.
use baedeker_core::binary::module::Module;
use baedeker_core::validate::validate_module;

fn uleb(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}
fn section(id: u8, payload: &[u8]) -> Vec<u8> {
    let mut s = vec![id];
    uleb(payload.len() as u64, &mut s);
    s.extend_from_slice(payload);
    s
}
fn module(sections: &[Vec<u8>]) -> Vec<u8> {
    let mut m = vec![0x00, b'a', b's', b'm', 0x01, 0x00, 0x00, 0x00];
    for s in sections {
        m.extend_from_slice(s);
    }
    m
}

#[test]
fn probe_atomic_in_global_init_is_non_constant() {
    // global (mut i32), init = i32.atomic.load — must NOT validate.
    let mut init = vec![0x41, 0x00, 0xFE, 0x10, 0x02, 0x00, 0x0B];
    let global_payload = [0x01, 0x7F, 0x01]; // 1 global, i32, mutable
    let bytes = module(&[section(6, &global_payload)]);
    let mut with_code = bytes.clone();
    // simpler: global section with explicit init: count, valtype, mut, expr
    let mut g = vec![0x01, 0x7F, 0x00];
    g.append(&mut init);
    with_code = module(&[section(6, &g)]);
    let m = Module::decode(&with_code).expect("decodes (const-expr is bytes)");
    let err = validate_module(&m).expect_err("atomic in const expr must be rejected");
    assert!(err.to_string().contains("constant"), "got: {err}");
}

#[test]
fn probe_limits_0x06_refused() {
    // shared+64 combo does not exist at the pin: 0x06/0x07 malformed.
    let bytes = module(&[section(5, &[0x01, 0x06, 0x01])]);
    assert!(Module::decode(&bytes).is_err(), "0x06 must be refused");
    let bytes = module(&[section(5, &[0x01, 0x07, 0x01, 0x02])]);
    assert!(Module::decode(&bytes).is_err(), "0x07 must be refused");
}

#[test]
fn probe_shared_import_links_roundtrip_validate() {
    // A shared memory import module validates standalone (provider is
    // host-side; validation only checks the declared type is well-formed).
    let payload: &[u8] = &[
        0x01, 0x01, b'e', 0x01, b'm', 0x02, 0x03, 0x01,
        0x02,
        // func using it: type ()->(), body: (i32.const 0) i32.atomic.load drop
    ];
    let func_type = section(1, &[0x01, 0x60, 0x00, 0x00]);
    let code = {
        let mut c = vec![0x01, 0x09];
        c.extend_from_slice(&[0x00, 0x41, 0x00, 0xFE, 0x10, 0x02, 0x00, 0x1A, 0x0B]);
        section(10, &c)
    };
    let func = section(3, &[0x01, 0x00]);
    let bytes = module(&[func_type, section(2, payload), func, code]);
    let m = Module::decode(&bytes).expect("decodes");
    validate_module(&m).expect("shared import + atomic load validates");
}
