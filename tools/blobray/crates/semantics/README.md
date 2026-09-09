# Vendor semantics layer

Architecture-neutral effect comparison types shared by the generic Blobray
engine and declarative add-ons. This crate contains no platform registry,
production driver dependency, or target-owned verdict callback.

A disposition may declare an empty exact effect contract only with explicit
return comparison. This represents a computation without side effects:
both executions must return the same value and any observed side effect is
unclassified. An empty policy never proves return equality on its own.
