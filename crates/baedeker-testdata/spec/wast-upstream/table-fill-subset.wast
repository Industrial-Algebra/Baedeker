;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/bulk-memory/table_fill.wast

(module
  (table $t0 10 externref)
  (table $t1 4 funcref)

  (func (export "fill-externref") (param $i i32) (param $r externref) (param $n i32)
    (table.fill $t0 (local.get $i) (local.get $r) (local.get $n)))
  (func (export "fill-funcref") (param $i i32) (param $n i32)
    (table.fill $t1 (local.get $i) (ref.null func) (local.get $n))))

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-index-value-length-empty-vs-i32-i32
      (table.fill $t)))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-index-empty-vs-i32
      (table.fill $t (ref.null extern) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-value-empty-vs
      (table.fill $t (i32.const 1) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 10 externref)
    (func $type-length-empty-vs-i32
      (table.fill $t (i32.const 1) (ref.null extern))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 0 funcref)
    (func $type-value-vs-funcref (param $r externref)
      (table.fill $t (i32.const 1) (local.get $r) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t1 1 externref)
    (table $t2 1 funcref)
    (func $type-value-externref-vs-funcref-multi (param $r externref)
      (table.fill $t2 (i32.const 0) (local.get $r) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (table $t 1 externref)
    (func $type-result-empty-vs-num (result i32)
      (table.fill $t (i32.const 0) (ref.null extern) (i32.const 1))))
  "type mismatch"
)
