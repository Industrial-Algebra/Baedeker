;; Control flow: loop execution cohort.

(module
  ;; Classic countdown accumulator: sum n + (n-1) + ... + 1.
  ;; Exercises the loop back-edge (br 0) and loop exit via br_if 1.
  (func (export "loop_sum") (param i32) (result i32)
    (local i32)
    block
      loop
        local.get 0
        i32.eqz
        br_if 1
        local.get 1
        local.get 0
        i32.add
        local.set 1
        local.get 0
        i32.const 1
        i32.sub
        local.set 0
        br 0
      end
    end
    local.get 1)

  ;; Loop exit taken before the body runs when the condition holds on
  ;; entry; otherwise the body runs exactly once.
  (func (export "loop_zero_trip") (param i32) (result i32)
    (local i32)
    block
      loop
        local.get 0
        br_if 1
        i32.const 99
        local.set 1
        i32.const 1
        local.set 0
        br 0
      end
    end
    local.get 1)

  ;; br carrying a value out of a loop to an enclosing block.
  (func (export "loop_br_value") (param i32) (result i32)
    block (result i32)
      loop
        local.get 0
        i32.eqz
        if (result i32)
          i32.const 42
          br 2
        else
          i32.const 0
        end
        drop
        local.get 0
        i32.const 1
        i32.sub
        local.set 0
        br 0
      end
      i32.const 0
    end)

  ;; Loop with a result value produced by falling through the end.
  (func (export "loop_result") (result i32)
    loop (result i32)
      i32.const 5
    end)
)

(assert_return (invoke "loop_sum" (i32.const 0)) (i32.const 0))
(assert_return (invoke "loop_sum" (i32.const 1)) (i32.const 1))
(assert_return (invoke "loop_sum" (i32.const 5)) (i32.const 15))
(assert_return (invoke "loop_sum" (i32.const 10)) (i32.const 55))
(assert_return (invoke "loop_zero_trip" (i32.const 7)) (i32.const 0))
(assert_return (invoke "loop_zero_trip" (i32.const 0)) (i32.const 99))
(assert_return (invoke "loop_br_value" (i32.const 0)) (i32.const 42))
(assert_return (invoke "loop_br_value" (i32.const 3)) (i32.const 42))
(assert_return (invoke "loop_result") (i32.const 5))
