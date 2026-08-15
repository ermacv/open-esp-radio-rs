# C semantic add-on

This add-on identifies standardized C runtime boundaries by exact public
symbol name. Architecture backends may apply the declared ABI contract to a
call site, but must not use the callee implementation body as semantic
evidence.

The add-on is optional. A generic Workbench build or a non-C project receives
none of these contracts unless its compiled knowledge provider composes this
add-on explicitly.

Only exact fixed-arity contracts are selected. Variadic formatting functions
are not coerced into a fixed signature: they remain explicit blockers until a
reviewed variadic ABI and output-effect model is available.
