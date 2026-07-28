;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/br_if.wast

(module
  (func $dummy)

  (func (export "type-i32-value") (result i32)
    (block (result i32) (i32.ctz (br_if 0 (i32.const 1) (i32.const 1)))))
  (func (export "type-i64-value") (result i64)
    (block (result i64) (i64.ctz (br_if 0 (i64.const 2) (i32.const 1)))))
  (func (export "type-f32-value") (result f32)
    (block (result f32) (f32.neg (br_if 0 (f32.const 3) (i32.const 1)))))
  (func (export "type-f64-value") (result f64)
    (block (result f64) (f64.neg (br_if 0 (f64.const 4) (i32.const 1)))))

  (func (export "as-block-first") (param i32) (result i32)
    (block (br_if 0 (local.get 0)) (return (i32.const 2)))
    (i32.const 3))
  (func (export "as-block-mid") (param i32) (result i32)
    (block (call $dummy) (br_if 0 (local.get 0)) (return (i32.const 2)))
    (i32.const 3))
  (func (export "as-block-last") (param i32)
    (block (call $dummy) (call $dummy) (br_if 0 (local.get 0))))
  (func (export "as-block-first-value") (param i32) (result i32)
    (block (result i32)
      (drop (br_if 0 (i32.const 10) (local.get 0)))
      (return (i32.const 11))))
  (func (export "as-block-mid-value") (param i32) (result i32)
    (block (result i32)
      (call $dummy)
      (drop (br_if 0 (i32.const 20) (local.get 0)))
      (return (i32.const 21))))
  (func (export "as-block-last-value") (param i32) (result i32)
    (block (result i32)
      (call $dummy) (call $dummy) (br_if 0 (i32.const 11) (local.get 0))))

  (func (export "as-loop-first") (param i32) (result i32)
    (block (loop (br_if 1 (local.get 0)) (return (i32.const 2))))
    (i32.const 3))
  (func (export "as-loop-mid") (param i32) (result i32)
    (block (loop (call $dummy) (br_if 1 (local.get 0)) (return (i32.const 2))))
    (i32.const 4))
  (func (export "as-loop-last") (param i32)
    (loop (call $dummy) (br_if 1 (local.get 0))))

  (func (export "as-br-value") (result i32)
    (block (result i32) (br 0 (br_if 0 (i32.const 1) (i32.const 2)))))

  (func (export "as-br_if-cond")
    (block (br_if 0 (br_if 0 (i32.const 1) (i32.const 1)))))
  (func (export "as-br_if-value") (result i32)
    (block (result i32)
      (drop (br_if 0 (br_if 0 (i32.const 1) (i32.const 2)) (i32.const 3)))
      (i32.const 4)))
  (func (export "as-br_if-value-cond") (param i32) (result i32)
    (block (result i32)
      (drop (br_if 0 (i32.const 2) (br_if 0 (i32.const 1) (local.get 0))))
      (i32.const 4)))

  (func (export "as-br_table-index")
    (block (br_table 0 0 0 (br_if 0 (i32.const 1) (i32.const 2)))))
  (func (export "as-br_table-value") (result i32)
    (block (result i32)
      (br_table 0 0 0 (br_if 0 (i32.const 1) (i32.const 2)) (i32.const 3))
      (i32.const 4)))
  (func (export "as-br_table-value-index") (result i32)
    (block (result i32)
      (br_table 0 0 (i32.const 2) (br_if 0 (i32.const 1) (i32.const 3)))
      (i32.const 4)))
  (func (export "as-return-value") (result i64)
    (block (result i64) (return (br_if 0 (i64.const 1) (i32.const 2)))))

  (func (export "as-if-cond") (param i32) (result i32)
    (block (result i32)
      (if (result i32)
        (br_if 0 (i32.const 1) (local.get 0))
        (then (i32.const 2))
        (else (i32.const 3)))))
  (func (export "as-if-then") (param i32 i32)
    (block
      (if (local.get 0) (then (br_if 1 (local.get 1))) (else (call $dummy)))))
  (func (export "as-if-else") (param i32 i32)
    (block
      (if (local.get 0) (then (call $dummy)) (else (br_if 1 (local.get 1))))))

  (func (export "as-select-first") (param i32) (result i32)
    (block (result i32)
      (select (br_if 0 (i32.const 3) (i32.const 10)) (i32.const 2) (local.get 0))))
  (func (export "as-select-second") (param i32) (result i32)
    (block (result i32)
      (select (i32.const 1) (br_if 0 (i32.const 3) (i32.const 10)) (local.get 0))))
  (func (export "as-select-cond") (result i32)
    (block (result i32)
      (select (i32.const 1) (i32.const 2) (br_if 0 (i32.const 3) (i32.const 10)))))

  (func $f (param i32 i32 i32) (result i32)
    (i32.const -1))
  (func (export "as-call-first") (result i32)
    (block (result i32)
      (call $f
        (br_if 0 (i32.const 12) (i32.const 1))
        (i32.const 2)
        (i32.const 3))))
  (func (export "as-call-mid") (result i32)
    (block (result i32)
      (call $f
        (i32.const 1)
        (br_if 0 (i32.const 13) (i32.const 1))
        (i32.const 3))))
  (func (export "as-call-last") (result i32)
    (block (result i32)
      (call $f
        (i32.const 1)
        (i32.const 2)
        (br_if 0 (i32.const 14) (i32.const 1)))))

  (func $func (param i32 i32 i32) (result i32)
    (local.get 0))
  (type $check (func (param i32 i32 i32) (result i32)))
  (table funcref (elem $func))
  (func (export "as-call_indirect-func") (result i32)
    (block (result i32)
      (call_indirect (type $check)
        (br_if 0 (i32.const 4) (i32.const 10))
        (i32.const 1)
        (i32.const 2)
        (i32.const 0))))
  (func (export "as-call_indirect-first") (result i32)
    (block (result i32)
      (call_indirect (type $check)
        (i32.const 1)
        (br_if 0 (i32.const 4) (i32.const 10))
        (i32.const 2)
        (i32.const 0))))
  (func (export "as-call_indirect-mid") (result i32)
    (block (result i32)
      (call_indirect (type $check)
        (i32.const 1)
        (i32.const 2)
        (br_if 0 (i32.const 4) (i32.const 10))
        (i32.const 0))))
  (func (export "as-call_indirect-last") (result i32)
    (block (result i32)
      (call_indirect (type $check)
        (i32.const 1)
        (i32.const 2)
        (i32.const 3)
        (br_if 0 (i32.const 4) (i32.const 10)))))

  (func (export "as-local.set-value") (param i32) (result i32)
    (local i32)
    (block (result i32)
      (local.set 0 (br_if 0 (i32.const 17) (local.get 0)))
      (i32.const -1)))
  (func (export "as-local.tee-value") (param i32) (result i32)
    (block (result i32)
      (local.tee 0 (br_if 0 (i32.const 1) (local.get 0)))
      (return (i32.const -1))))
  (global $a (mut i32) (i32.const 10))
  (func (export "as-global.set-value") (param i32) (result i32)
    (block (result i32)
      (global.set $a (br_if 0 (i32.const 1) (local.get 0)))
      (return (i32.const -1))))

  (memory 1)
  (func (export "as-load-address") (result i32)
    (block (result i32) (i32.load (br_if 0 (i32.const 1) (i32.const 1)))))
  (func (export "as-loadN-address") (result i32)
    (block (result i32) (i32.load8_s (br_if 0 (i32.const 30) (i32.const 1)))))
  (func (export "as-store-address") (result i32)
    (block (result i32)
      (i32.store (br_if 0 (i32.const 30) (i32.const 1)) (i32.const 7))
      (i32.const -1)))
  (func (export "as-store-value") (result i32)
    (block (result i32)
      (i32.store (i32.const 2) (br_if 0 (i32.const 31) (i32.const 1)))
      (i32.const -1)))
  (func (export "as-storeN-address") (result i32)
    (block (result i32)
      (i32.store8 (br_if 0 (i32.const 32) (i32.const 1)) (i32.const 7))
      (i32.const -1)))
  (func (export "as-storeN-value") (result i32)
    (block (result i32)
      (i32.store16 (i32.const 2) (br_if 0 (i32.const 33) (i32.const 1)))
      (i32.const -1)))

  (func (export "as-unary-operand") (result f64)
    (block (result f64) (f64.neg (br_if 0 (f64.const 1.0) (i32.const 1)))))
  (func (export "as-binary-left") (result i32)
    (block (result i32) (i32.add (br_if 0 (i32.const 1) (i32.const 1)) (i32.const 10))))
  (func (export "as-binary-right") (result i32)
    (block (result i32) (i32.sub (i32.const 10) (br_if 0 (i32.const 1) (i32.const 1)))))
  (func (export "as-test-operand") (result i32)
    (block (result i32) (i32.eqz (br_if 0 (i32.const 0) (i32.const 1)))))
  (func (export "as-compare-left") (result i32)
    (block (result i32) (i32.le_u (br_if 0 (i32.const 1) (i32.const 1)) (i32.const 10))))
  (func (export "as-compare-right") (result i32)
    (block (result i32) (i32.ne (i32.const 10) (br_if 0 (i32.const 1) (i32.const 42)))))

  (func (export "as-memory.grow-size") (result i32)
    (block (result i32) (memory.grow (br_if 0 (i32.const 1) (i32.const 1)))))
)

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param i32) (result (ref null $t1))
    (block (result (ref null $t1))
      (br_if 0 (ref.func $f) (local.get 0)))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param i32) (result (ref null $t1))
      (block (result (ref null $t1))
        (br_if 0 (ref.func $f) (local.get 0)))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t1)
    (local.get 0))
  (export "f" (func $f))
  (func (result (ref null $t0))
    (block (result (ref null $t0))
      (ref.null $t0)
      (loop (param (ref null $t0)) (result (ref null $t0))
        (drop)
        (br_if 0 (ref.func $f) (i32.const 1))))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t1)
      (local.get 0))
    (export "f" (func $f))
    (func (result (ref null $t0))
      (block (result (ref null $t0))
        (ref.null $t0)
        (loop (param (ref null $t0)) (result (ref null $t0))
          (drop)
          (br_if 0 (ref.func $f) (i32.const 1))))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param $c i32) (param $s i32) (result (ref null $t0))
    (block (result (ref null $t0))
      (br_if 0
        (select (result (ref null $t0))
          (ref.func $f)
          (ref.null $t0)
          (local.get $s))
        (local.get $c)))))

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (global $g0 (ref null $t0)
    (ref.func $f))
  (table $t 1 (ref null $t0))
  (elem (ref null $t0)
    (global.get $g0))
  (func (param $c i32) (result (ref null $t0))
    (block (result (ref null $t0))
      (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
      (drop
        (br_if 0
          (table.get $t (i32.const 0))
          (local.get $c)))
      (ref.null $t0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $callee (func (result (ref null $t0))))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func $g (type $callee)
    (ref.null $t0))
  (global $gref (ref null $callee)
    (ref.func $g))
  (table $t 1 (ref null $callee))
  (elem (ref null $callee)
    (global.get $gref))
  (func (param $c i32) (result (ref null $callee))
    (local $r (ref null $callee))
    (block (result (ref null $callee))
      (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
      (local.set $r
        (table.get $t (i32.const 0)))
      (drop
        (br_if 0
          (local.get $r)
          (local.get $c)))
      (local.get $r))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $callee (func (result (ref null $t0))))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func $g (type $callee)
      (ref.null $t0))
    (global $gref (ref null $callee)
      (ref.func $g))
    (table $t 1 (ref null $callee))
    (elem (ref null $callee)
      (global.get $gref))
    (func (param $c i32) (result (ref $callee))
      (local $r (ref null $callee))
      (block (result (ref $callee))
        (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
        (local.set $r
          (table.get $t (i32.const 0)))
        (drop
          (br_if 0
            (local.get $r)
            (local.get $c)))
        (ref.func $g))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param $c i32) (param $s i32) (result (ref $t0))
      (block (result (ref $t0))
        (br_if 0
          (select (result (ref null $t0))
            (ref.func $f)
            (ref.null $t0)
            (local.get $s))
          (local.get $c)))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (global $g0 (ref null $t0)
      (ref.func $f))
    (table $t 1 (ref null $t0))
    (elem (ref null $t0)
      (global.get $g0))
    (func (param $c i32) (result (ref $t0))
      (block (result (ref $t0))
        (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
        (drop
          (br_if 0
            (table.get $t (i32.const 0))
            (local.get $c)))
        (ref.func $f))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param $c i32) (param $s i32) (result (ref null $t0))
    (block (result (ref null $t0))
      (ref.null $t0)
      (loop (param (ref null $t0)) (result (ref null $t0))
        (drop)
        (br_if 0
          (select (result (ref null $t0))
            (ref.func $f)
            (ref.null $t0)
            (local.get $s))
          (local.get $c))))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param $c i32) (param $s i32) (result (ref $t0))
      (block (result (ref $t0))
        (ref.func $f)
        (loop (param (ref $t0)) (result (ref $t0))
          (drop)
          (br_if 0
            (select (result (ref null $t0))
              (ref.func $f)
              (ref.null $t0)
              (local.get $s))
            (local.get $c))))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-false-i32 (block (i32.ctz (br_if 0 (i32.const 0))))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-true-i64 (block (i64.ctz (br_if 0 (i64.const 1))))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-false-arg-void-vs-num (result i32)
    (block (result i32) (br_if 0 (i32.const 0)) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-true-arg-void-vs-num (result i32)
    (block (result i32) (br_if 0 (i32.const 1)) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-false-arg-num-vs-void
    (block (br_if 0 (i32.const 0) (i32.const 0)))))
  "type mismatch"
)
