;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/multi-memory/data_drop0.wast

(module
  (memory $m0 0)
  (memory $m1 1)
  (memory $m2 0)
  (data $passive "x")
  (data $active (memory $m1) (i32.const 0) "x")

  (func (export "drop-passive")
    (data.drop $passive))
  (func (export "init-passive") (param $len i32)
    (memory.init $m1 $passive (i32.const 0) (i32.const 0) (local.get $len)))

  (func (export "drop-active")
    (data.drop $active))
  (func (export "init-active") (param $len i32)
    (memory.init $m1 $active (i32.const 0) (i32.const 0) (local.get $len))))

(assert_invalid
  (module
    (data "x")
    (func
      (data.drop 1)))
  "unknown data segment"
)
