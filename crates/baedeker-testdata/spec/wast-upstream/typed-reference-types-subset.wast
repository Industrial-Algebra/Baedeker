;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/call_ref.wast

(module
  (type $t (func))

  (func
    (param
      funcref
      externref
      (ref func)
      (ref extern)
      (ref 0)
      (ref $t)
      (ref 0)
      (ref $t)
      (ref null func)
      (ref null extern)
      (ref null 0)
      (ref null $t)
    )
  )
)

(module
  (type $ii (func (param i32) (result i32)))

  (func $f (type $ii)
    (local.get 0))
  (export "f" (func $f))

  (global (mut (ref null $ii))
    (ref.null $ii))

  (global (ref null $ii)
    (ref.func $f))

  (func (export "id") (param (ref $ii)) (result (ref null $ii))
    (local.get 0)))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (global (ref null $t1)
    (ref.func $f)))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (import "env" "g" (global (ref null $t0)))
  (global (ref null $t1)
    (global.get 0)))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (global $g0 (ref null $t0)
    (ref.func $f))
  (global (ref null $t1)
    (global.get $g0)))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (import "env" "g" (global (mut (ref null $t1))))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func
    (global.set 0 (ref.func $f))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (table 1 (ref null $t0))
  (global (mut (ref null $t1)) (ref.null $t1))
  (func (result (ref null $t1))
    (table.set 0 (i32.const 0) (ref.func $f))
    (global.set 0 (table.get 0 (i32.const 0)))
    (global.get 0)))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (table 1 (ref null $t0))
    (global (mut (ref null $t1)) (ref.null $t1))
    (func (result (ref null $t1))
      (table.set 0 (i32.const 0) (ref.func $f))
      (global.set 0 (table.get 0 (i32.const 0)))
      (global.get 0)))
  "type mismatch"
)

(assert_invalid
  (module
    (type $ii (func (param i32) (result i32)))
    (func $f (type $ii) (local.get 0))
    (global (ref null $ii)
      (ref.null extern)))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i64)))
    (import "env" "g" (global (ref null $t0)))
    (global (ref null $t1)
      (global.get 0)))
  "type mismatch"
)
