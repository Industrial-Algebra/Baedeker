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
