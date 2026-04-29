;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/table_get.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/table_set.wast

(module
  (table $t2 2 externref)
  (table $t3 3 funcref)
  (elem (table $t3) (i32.const 1) func $dummy)
  (func $dummy)

  (func (export "init") (param $r externref)
    (table.set $t2 (i32.const 1) (local.get $r))
    (table.set $t3 (i32.const 2) (table.get $t3 (i32.const 1))))

  (func (export "externref-roundtrip") (param $i i32) (param $r externref)
    (table.set $t2 (local.get $i) (local.get $r))
    (drop (table.get $t2 (local.get $i))))

  (func (export "funcref-roundtrip") (param $i i32) (param $j i32) (result i32)
    (table.set $t3 (local.get $i) (table.get $t3 (local.get $j)))
    (ref.is_null (table.get $t3 (local.get $i)))))

(assert_invalid
  (module
    (table $t0 1 externref)
    (table $t1 1 funcref)
    (func
      (table.set $t1 (i32.const 0) (table.get $t0 (i32.const 0)))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t0 1 externref)
    (table $t1 1 funcref)
    (func (result funcref)
      (table.get $t0 (i32.const 0))))
  "type mismatch"
)
