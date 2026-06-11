;; Control flow: block and br execution cohort.

(module
  ;; Simple empty block
  (func (export "empty_block") (result i32)
    block
    end
    i32.const 42)

  ;; br 0 skips, then produces result
  (func (export "br_skip") (result i32)
    block
      br 0
    end
    i32.const 99)

  ;; Multiple branches skip to end
  (func (export "br_skip_chain") (result i32)
    block
      block
        br 1
      end
    end
    i32.const 7))

(assert_return (invoke "empty_block") (i32.const 42))
(assert_return (invoke "br_skip") (i32.const 99))
(assert_return (invoke "br_skip_chain") (i32.const 7))
