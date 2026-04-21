;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/bulk-memory/table_copy.wast

(module
  (type (func (result i32)))
  (table $t0 10 funcref)
  (table $t1 10 funcref)
  (elem (table $t0) (i32.const 2) func 0 1)
  (elem (table $t1) (i32.const 3) func 1 0)
  (func (result i32) (i32.const 0))
  (func (result i32) (i32.const 1))
  (func (export "copy-same")
    (table.copy $t0 $t0 (i32.const 4) (i32.const 2) (i32.const 2)))
  (func (export "copy-cross")
    (table.copy $t0 $t1 (i32.const 6) (i32.const 3) (i32.const 2))))
