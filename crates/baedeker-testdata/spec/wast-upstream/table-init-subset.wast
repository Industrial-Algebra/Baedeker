;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/bulk-memory/table_init.wast

(module
  (table $t0 10 funcref)
  (table $t1 10 funcref)
  (elem (table $t0) (i32.const 2) func 0)
  (elem funcref (ref.func 0) (ref.func 1))
  (func (result i32) (i32.const 0))
  (func (result i32) (i32.const 1))
  (func (export "init-nonzero-elem")
    (table.init $t0 1 (i32.const 7) (i32.const 0) (i32.const 2)))
  (func (export "init-nonzero-table")
    (table.init $t1 1 (i32.const 3) (i32.const 0) (i32.const 1)))
  (func (export "drop-elem")
    (elem.drop 1)))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (table 4 (ref null $t0))
  (elem (ref null $t1) (ref.func $f))
  (func (export "init")
    (table.init 0 0 (i32.const 0) (i32.const 0) (i32.const 1))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (global $g0 (ref null $t0)
    (ref.func $f))
  (table 1 (ref null $t1))
  (elem (ref null $t1) (global.get $g0))
  (func (result (ref null $t1))
    (table.init 0 0 (i32.const 0) (i32.const 0) (i32.const 1))
    (table.get 0 (i32.const 0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (import "env" "g" (global (ref null $t0)))
  (table 1 (ref null $t1))
  (elem (ref null $t1) (global.get 0))
  (func (result (ref null $t1))
    (table.init 0 0 (i32.const 0) (i32.const 0) (i32.const 1))
    (table.get 0 (i32.const 0))))

(assert_invalid
  (module
    (func (export "test")
      (table.init 0 (i32.const 12) (i32.const 1) (i32.const 1))))
  "unknown table 0"
)

(assert_invalid
  (module
    (elem funcref (ref.func 0))
    (func (result i32) (i32.const 0))
    (func (export "test")
      (elem.drop 4)))
  "unknown elem segment 4"
)

(assert_invalid
  (module
    (table 10 funcref)
    (elem funcref (ref.func $f0) (ref.func $f0) (ref.func $f0))
    (func $f0)
    (func (export "test")
      (table.init 0 (i32.const 1) (i32.const 1) (f32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table 10 funcref)
    (elem funcref (ref.func $f0) (ref.func $f0) (ref.func $f0))
    (func $f0)
    (func (export "test")
      (table.init 0 (i32.const 1) (f32.const 1) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i64)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (table 4 (ref null $t1))
    (elem (ref null $t0) (ref.func $f))
    (func (export "init")
      (table.init 0 0 (i32.const 0) (i32.const 0) (i32.const 1))))
  "type mismatch"
)
