;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/table.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/call_ref.wast

(module
  (type $ii (func (param i32) (result i32)))

  (func $f (type $ii)
    (local.get 0))
  (export "f" (func $f))

  (table $t 1 (ref null $ii))

  (func (result (ref null $ii))
    (table.set $t (i32.const 0) (ref.func $f))
    (table.get $t (i32.const 0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (table $t 1 (ref null $t1))

  (func (result (ref null $t1))
    (table.set $t (i32.const 0) (ref.func $f))
    (table.get $t (i32.const 0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (import "env" "g" (global (ref null $t0)))
  (table 1 (ref null $t1))
  (func (result (ref null $t1))
    (table.set 0 (i32.const 0) (global.get 0))
    (table.get 0 (i32.const 0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (import "env" "g" (global (ref null $t0)))
  (import "env" "t" (table 1 (ref null $t1)))
  (func (result (ref null $t1))
    (table.set 0 (i32.const 0) (global.get 0))
    (table.get 0 (i32.const 0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (import "env" "t" (table 1 (ref null $t0)))
  (global (mut (ref null $t1)) (ref.null $t1))
  (func (result (ref null $t1))
    (global.set 0 (table.get 0 (i32.const 0)))
    (global.get 0)))

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (global $g (mut (ref null $t0)) (ref.null $t0))
  (func (param $c i32)
    (global.set $g
      (if (result (ref null $t0))
        (local.get $c)
        (then
          (ref.func $f))
        (else
          (ref.null $t0))))))

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (table $t 1 (ref null $t0))
  (func (param $c i32)
    (table.set $t
      (i32.const 0)
      (if (result (ref null $t0))
        (local.get $c)
        (then
          (ref.func $f))
        (else
          (ref.null $t0))))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (import "env" "t" (table 1 (ref null $t0)))
    (global (mut (ref null $t1)) (ref.null $t1))
    (func (result (ref null $t1))
      (global.set 0 (table.get 0 (i32.const 0)))
      (global.get 0)))
  "type mismatch"
)

(assert_invalid
  (module
    (type $ii (func (param i32) (result i32)))
    (func $f (type $ii) (local.get 0))
    (export "f" (func $f))
    (table $t 1 (ref null $ii))
    (func
      (table.set $t (i32.const 0) (ref.null extern))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (global $g (mut (ref $t0)) (ref.func $f))
    (func (param $c i32)
      (global.set $g
        (if (result (ref null $t0))
          (local.get $c)
          (then
            (ref.func $f))
          (else
            (ref.null $t0))))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (table $t 1 (ref $t0))
    (func (param $c i32)
      (table.set $t
        (i32.const 0)
        (if (result (ref null $t0))
          (local.get $c)
          (then
            (ref.func $f))
          (else
            (ref.null $t0))))))
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
    (global $sink (mut (ref $t0))
      (ref.func $f))
    (func
      (table.init $t 0 (i32.const 0) (i32.const 0) (i32.const 1))
      (global.set $sink
        (table.get $t (i32.const 0)))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i64)))
    (import "env" "g" (global (ref null $t0)))
    (import "env" "t" (table 1 (ref null $t1)))
    (func (result (ref null $t1))
      (table.set 0 (i32.const 0) (global.get 0))
      (table.get 0 (i32.const 0))))
  "type mismatch"
)
