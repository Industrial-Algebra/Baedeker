;; Control flow: br_if execution cohort.

(module
  ;; br_if: conditional branch taken
  (func (export "br_if_taken") (result i32)
    block
      i32.const 1
      br_if 0
      i32.const 0
      return
    end
    i32.const 42)

  ;; br_if: conditional branch not taken
  (func (export "br_if_not_taken") (result i32)
    block
      i32.const 0
      br_if 0
      i32.const 99
      return
    end
    i32.const 0))

(assert_return (invoke "br_if_taken") (i32.const 42))
(assert_return (invoke "br_if_not_taken") (i32.const 99))
