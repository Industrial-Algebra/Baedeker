;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/call_ref.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i64) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $x i32) (result i32)
    (call_ref $t0 (local.get $x) (ref.func $f)))

  (func (param $x i32) (result i32)
    (call_ref $t0 (local.get $x) (ref.null $t0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $x i32) (result i32)
    (call_ref $t1 (local.get $x) (ref.func $f))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $x i32) (result i32)
    (call_ref $t1
      (local.get $x)
      (block (result (ref null $t0))
        (ref.func $f)))))

(module
  (type $ii (func (param i32) (result i32)))

  (func $apply (param $f (ref $ii)) (param $x i32) (result i32)
    (call_ref $ii (local.get $x) (local.get $f)))

  (func $f (type $ii)
    (i32.mul (local.get 0) (local.get 0)))
  (func $g (type $ii)
    (i32.sub (i32.const 0) (local.get 0)))

  (elem declare func $f $g)

  (func (export "run") (param $x i32) (result i32)
    (local $rf (ref null $ii))
    (local $rg (ref null $ii))
    (local.set $rf (ref.func $f))
    (local.set $rg (ref.func $g))
    (call_ref $ii
      (call_ref $ii (local.get $x) (local.get $rf))
      (local.get $rg))))

(module
  (type $ll (func (param i64) (result i64)))

  (elem declare func $fac)
  (global $fac (ref $ll) (ref.func $fac))

  (func $fac (export "fac") (type $ll)
    (if (result i64) (i64.eqz (local.get 0))
      (then (i64.const 1))
      (else
        (i64.mul
          (local.get 0)
          (call_ref $ll
            (i64.sub (local.get 0) (i64.const 1))
            (global.get $fac)))))))

(module
  (elem declare func $f)
  (type $t (func (param i32) (result i32)))
  (func $f (param i32) (result i32) (local.get 0))

  (func (export "unreachable-ref-func") (result i32)
    (unreachable)
    (ref.func $f)
    (call_ref $t))

  (func (export "unreachable-call-drop") (result i32)
    (unreachable)
    (i32.const 0)
    (ref.func $f)
    (call_ref $t)
    (drop)
    (i32.const 0)))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (func $f (type $t0) (local.get 0))
    (export "f" (func $f))
    (func (param $x i64) (result i32)
      (call_ref $t1 (local.get $x) (ref.func $f))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param $x i64) (result i32)
      (call_ref $t1
        (local.get $x)
        (block (result (ref null $t0))
          (ref.func $f)))))
  "type mismatch"
)

(assert_invalid
  (module
    (elem declare func $f)
    (type $t (func (param i32) (result i32)))
    (func $f (param i32) (result i32) (local.get 0))

    (func (export "unreachable-bad-arg") (result i32)
      (unreachable)
      (i64.const 0)
      (ref.func $f)
      (call_ref $t)))
  "type mismatch"
)

(assert_invalid
  (module
    (elem declare func $f)
    (type $t (func (param i32) (result i32)))
    (func $f (param i32) (result i32) (local.get 0))

    (func (export "unreachable-bad-tail") (result i32)
      (unreachable)
      (ref.func $f)
      (call_ref $t)
      (drop)
      (i64.const 0)))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t (func))
    (func $f (param $r externref)
      (call_ref $t (local.get $r))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t (func))
    (func $f (param $r funcref)
      (call_ref $t (local.get $r))))
  "type mismatch"
)
