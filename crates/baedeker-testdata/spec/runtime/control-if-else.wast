;; Control flow: if/else execution cohort.

(module
  ;; Binary selection on the condition.
  (func (export "if_else") (param i32) (result i32)
    local.get 0
    if (result i32)
      i32.const 1
    else
      i32.const 2
    end)

  ;; if without else: then-body runs only when cond is non-zero.
  (func (export "if_no_else") (param i32) (result i32)
    (local i32)
    local.get 0
    if
      i32.const 42
      local.set 1
    end
    local.get 1)

  ;; Nested if inside then-body and else-body.
  (func (export "if_nested") (param i32 i32) (result i32)
    local.get 0
    if (result i32)
      local.get 1
      if (result i32)
        i32.const 10
      else
        i32.const 20
      end
    else
      local.get 1
      if (result i32)
        i32.const 30
      else
        i32.const 40
      end
    end)

  ;; Multi-instruction bodies producing values from locals.
  (func (export "if_locals") (param i32) (result i32)
    (local i32)
    local.get 0
    if
      i32.const 3
      local.set 1
    else
      i32.const 5
      local.set 1
    end
    local.get 1
    i32.const 1
    i32.add)

  ;; br from inside an if-body to the if's own end.
  (func (export "if_br") (param i32) (result i32)
    block (result i32)
      local.get 0
      if (result i32)
        i32.const 1
        br 1
      else
        i32.const 2
      end
    end)

  ;; return inside an if-body; code after the if must still lower.
  (func (export "if_return") (param i32) (result i32)
    local.get 0
    if
      i32.const 10
      return
    end
    i32.const 20)

  ;; unreachable traps when reached.
  (func (export "if_unreachable") (param i32) (result i32)
    local.get 0
    if
      unreachable
    end
    i32.const 7)
)

(assert_return (invoke "if_else" (i32.const 1)) (i32.const 1))
(assert_return (invoke "if_else" (i32.const 0)) (i32.const 2))
(assert_return (invoke "if_no_else" (i32.const 1)) (i32.const 42))
(assert_return (invoke "if_no_else" (i32.const 0)) (i32.const 0))
(assert_return (invoke "if_nested" (i32.const 1) (i32.const 1)) (i32.const 10))
(assert_return (invoke "if_nested" (i32.const 1) (i32.const 0)) (i32.const 20))
(assert_return (invoke "if_nested" (i32.const 0) (i32.const 1)) (i32.const 30))
(assert_return (invoke "if_nested" (i32.const 0) (i32.const 0)) (i32.const 40))
(assert_return (invoke "if_locals" (i32.const 1)) (i32.const 4))
(assert_return (invoke "if_locals" (i32.const 0)) (i32.const 6))
(assert_return (invoke "if_br" (i32.const 1)) (i32.const 1))
(assert_return (invoke "if_br" (i32.const 0)) (i32.const 2))
(assert_return (invoke "if_return" (i32.const 1)) (i32.const 10))
(assert_return (invoke "if_return" (i32.const 0)) (i32.const 20))
(assert_return (invoke "if_unreachable" (i32.const 0)) (i32.const 7))
(assert_trap (invoke "if_unreachable" (i32.const 1)) "unreachable")
