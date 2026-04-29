;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/bulk-memory/memory_copy.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/multi-memory/memory_copy0.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/multi-memory/memory_copy1.wast

(module
  (memory $dst (data "\ff\11\44\ee"))
  (memory $scratch (data "\ee\22\55\ff"))
  (memory $unused (data "\dd\33\66\00"))
  (memory $src (data "\aa\bb\cc\dd"))

  (func (export "copy-same-memory") (param i32 i32 i32)
    (memory.copy $src $src (local.get 0) (local.get 1) (local.get 2)))
  (func (export "copy-cross-memory") (param i32 i32 i32)
    (memory.copy $dst $src (local.get 0) (local.get 1) (local.get 2))))

(assert_invalid
  (module
    (memory 1)
    (func
      (memory.copy 0 1 (i32.const 10) (i32.const 20) (i32.const 30))))
  "unknown memory"
)

(assert_invalid
  (module
    (memory 1)
    (func
      (memory.copy (i32.const 10) (i32.const 20) (f32.const 30))))
  "type mismatch"
)

(assert_invalid
  (module
    (memory 1)
    (func
      (memory.copy (f32.const 10) (i32.const 20) (i32.const 30))))
  "type mismatch"
)
