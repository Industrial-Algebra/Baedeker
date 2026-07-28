;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/memory.wast

(module
  (memory 1)
  (func (export "memsize") (result i32)
    memory.size)
)

(assert_invalid
  (module (data (i32.const 0) ""))
  "unknown memory"
)

(assert_invalid
  (module (func (drop (f32.load (i32.const 0)))))
  "unknown memory"
)

(assert_invalid
  (module (func (f32.store (i32.const 0) (f32.const 0))))
  "unknown memory"
)
