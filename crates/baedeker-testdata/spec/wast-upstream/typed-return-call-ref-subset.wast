;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/return_call_ref.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i64) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $x i32) (result i32)
    (return_call_ref $t0 (local.get $x) (ref.func $f)))

  (func (param $x i32) (result i32)
    (return_call_ref $t0 (local.get $x) (ref.null $t0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $x i32) (result i32)
    (return_call_ref $t1 (local.get $x) (ref.func $f))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $cond i32) (param $x i32) (result i32)
    (return_call_ref $t1
      (local.get $x)
      (if (result (ref null $t0))
        (local.get $cond)
        (then (ref.func $f))
        (else (ref.null $t0))))))

(module
  (type $i64-i64 (func (param i64) (result i64)))

  (elem declare func $count)
  (global $count (ref $i64-i64) (ref.func $count))

  (func $count (export "count") (param i64) (result i64)
    (if (result i64) (i64.eqz (local.get 0))
      (then (local.get 0))
      (else
        (return_call_ref $i64-i64
          (i64.sub (local.get 0) (i64.const 1))
          (global.get $count))))))

(module
  (type $i64-i64 (func (param i64) (result i64)))

  (global $even (ref $i64-i64) (ref.func $even))
  (global $odd (ref $i64-i64) (ref.func $odd))

  (elem declare func $even)
  (func $even (export "even") (param i64) (result i64)
    (if (result i64) (i64.eqz (local.get 0))
      (then (i64.const 44))
      (else
        (return_call_ref $i64-i64
          (i64.sub (local.get 0) (i64.const 1))
          (global.get $odd)))))

  (elem declare func $odd)
  (func $odd (export "odd") (param i64) (result i64)
    (if (result i64) (i64.eqz (local.get 0))
      (then (i64.const 99))
      (else
        (return_call_ref $i64-i64
          (i64.sub (local.get 0) (i64.const 1))
          (global.get $even))))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $callee (func (result (ref null $t0))))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func $g (type $callee)
    (ref.null $t0))
  (export "g" (func $g))
  (global $gref (ref null $callee)
    (ref.func $g))
  (table $t 1 (ref null $callee))
  (elem (ref null $callee)
    (global.get $gref))
  (func (result (ref null $t0))
    (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
    (return_call_ref $callee
      (table.get $t (i32.const 0)))))

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
  (func (result (ref null $t0))
    (local $r (ref null $callee))
    (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
    (local.set $r
      (table.get $t (i32.const 0)))
    (return_call_ref $callee
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
    (func (result (ref $t0))
      (local $r (ref null $callee))
      (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
      (local.set $r
        (table.get $t (i32.const 0)))
      (return_call_ref $callee
        (local.get $r))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t (func))
    (type $t2 (func (result (ref null $t))))
    (elem declare func $f22)
    (func $f12 (result (ref $t)) (return_call_ref $t2 (ref.func $f22)))
    (func $f22 (result (ref null $t)) (return_call_ref $t2 (ref.func $f22))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t (func))
    (type $t3 (func (result (ref func))))
    (elem declare func $f33)
    (func $f13 (result (ref $t)) (return_call_ref $t3 (ref.func $f33)))
    (func $f33 (result (ref func)) (return_call_ref $t3 (ref.func $f33))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t (func))
    (type $t4 (func (result (ref null func))))
    (elem declare func $f44)
    (func $f14 (result (ref $t)) (return_call_ref $t4 (ref.func $f44)))
    (func $f44 (result (ref null func)) (return_call_ref $t4 (ref.func $f44))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t (func))
    (type $t3 (func (result (ref func))))
    (elem declare func $f33)
    (func $f23 (result (ref null $t)) (return_call_ref $t3 (ref.func $f33)))
    (func $f33 (result (ref func)) (return_call_ref $t3 (ref.func $f33))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t (func))
    (type $t4 (func (result (ref null func))))
    (elem declare func $f44)
    (func $f24 (result (ref null $t)) (return_call_ref $t4 (ref.func $f44)))
    (func $f44 (result (ref null func)) (return_call_ref $t4 (ref.func $f44))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t4 (func (result (ref null func))))
    (elem declare func $f44)
    (func $f34 (result (ref func)) (return_call_ref $t4 (ref.func $f44)))
    (func $f44 (result (ref null func)) (return_call_ref $t4 (ref.func $f44))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (func $f (type $t0) (local.get 0))
    (export "f" (func $f))
    (func (param $x i64) (result i32)
      (return_call_ref $t1 (local.get $x) (ref.func $f))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param $cond i32) (param $x i64) (result i32)
      (return_call_ref $t1
        (local.get $x)
        (if (result (ref null $t0))
          (local.get $cond)
          (then (ref.func $f))
          (else (ref.null $t0))))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $callee (func (result (ref null $t0))))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func $g (type $callee)
      (ref.null $t0))
    (export "g" (func $g))
    (global $gref (ref null $callee)
      (ref.func $g))
    (table $t 1 (ref null $callee))
    (elem (ref null $callee)
      (global.get $gref))
    (func (result (ref $t0))
      (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
      (return_call_ref $callee
        (table.get $t (i32.const 0)))))
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
      (return_call_ref $t)))
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
      (return_call_ref $t)
      (i64.const 0)))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t (func))
    (func $f (param $r externref)
      (return_call_ref $t (local.get $r))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t (func))
    (func $f (param $r funcref)
      (return_call_ref $t (local.get $r))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $ty (func (result i32 i32)))
    (func (param (ref $ty)) (result i32)
      local.get 0
      return_call_ref $ty))
  "type mismatch"
)
