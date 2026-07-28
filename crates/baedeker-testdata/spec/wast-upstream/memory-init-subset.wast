;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/bulk-memory/memory_init.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/multi-memory/memory_init0.wast

(module
  (memory $scratch 0)
  (memory $dst 1)
  (data $passive "\aa\bb\cc\dd")
  (data $active (memory $dst) (i32.const 0) "\11")

  (func (export "init-passive-into-nonzero-memory")
    (memory.init $dst $passive (i32.const 4) (i32.const 1) (i32.const 2)))
  (func (export "init-active-into-nonzero-memory")
    (memory.init $dst $active (i32.const 8) (i32.const 0) (i32.const 1))))

(assert_invalid
  (module
    (memory 1)
    (data "x")
    (func
      (memory.init 1 0 (i32.const 0) (i32.const 0) (i32.const 1))))
  "unknown memory"
)

(assert_invalid
  (module
    (memory 1)
    (data "x")
    (func
      (memory.init 0 1 (i32.const 0) (i32.const 0) (i32.const 1))))
  "unknown data segment"
)

(assert_invalid
  (module
    (memory 1)
    (data "x")
    (func
      (memory.init 0 0 (i32.const 0) (f32.const 0) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (memory 1)
    (data "x")
    (func
      (memory.init 0 0 (i32.const 0) (i32.const 0) (f32.const 1))))
  "type mismatch"
)
