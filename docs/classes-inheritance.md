# Classes and Inheritance: Classes and `this`

> **Architecture case study:** adding class semantics by composing the structural model instead of creating a disconnected class type system.

## 1. Why classes came after structural types

Classes require declaration resolution, instance shape, inheritance, methods, constructors, and member access. Structural Types already provided object types, properties, functions, and a declaration namespace, so Classes and Inheritance could compose those primitives.

Fixtures cover inherited fields, missing inherited fields, method calls, constructor arity, `this`, and conservative unsupported-member behavior.

## 2. Class resolution

`TypeNamespace` resolves a class declaration into an object-shaped semantic representation. Resolution starts with inherited properties and then adds/updates the child's instance members.

```text
class declaration
      -> resolve heritage
      -> collect inherited properties
      -> resolve fields/methods
      -> build instance object shape
```

## 3. Why inheritance belongs in the namespace

Heritage is a declaration relationship. The namespace already owns name lookup and declaration resolution, so resolving a parent class there keeps expression inference focused on using the resulting type.

## 4. Methods reuse `FunctionType`

A method is callable. Instead of creating a separate method type, the checker reuses the existing function representation. Member access obtains the function type; normal call checking validates its arguments.

This gives:

```text
obj.method(...)
 -> member type
 -> callable check
 -> argument compatibility
```

## 5. Constructors

Constructor signatures reuse parameter metadata and are checked when a `new` expression is analyzed. The `new_expression_arity_mismatch.ts` fixture protects this behavior.

## 6. `this` context

`CheckContext` carries `current_class_instance: Option<TypeId>`. Class member checking temporarily enters a class context, resolves `this` against that context, and restores the previous context.

This is an explicit **context-stack style** design rather than a global special case.

## 7. Conservative unsupported behavior

Fixtures such as `this_expression_known_gap.ts`, `one_unresolvable_member_makes_whole_class_unsupported.ts`, and the untyped parameter cases document current boundaries. The checker prefers localized diagnostics or conservative fallback to corrupting the semantic model.

## 8. Why no separate `Type::Class`

A separate class type would force every operation to understand both `Object` and `Class`. Where instance semantics are structural, the existing `ObjectType` can represent the useful shape and therefore lets subtyping, properties, and calls reuse existing machinery.

## 9. What Classes and Inheritance completed

- class declaration resolution
- instance fields
- inherited fields
- methods
- constructors and arity
- method calls
- class-aware `this` context
- conservative unsupported-member handling

The architecture lesson is: **compose existing semantic primitives before introducing a new top-level type category.**

## 10. Where classes stand now

Measured against `tsc` 6.0.3 (see [language-coverage](language-coverage.md)): class members, inheritance, property initialization, parameter properties, accessors and generic classes are checked as `tsc` does, and classes cross files. The class rules that need no new type machinery are not enforced yet: instantiating an abstract class, reading a `private` or `#private` member from outside, a class that does not implement its interface, and an override with an incompatible type. The checker stays silent on those, and a `#private` field makes its class unchecked with a warning. One defect remains: reading a static member through the class name (`A.s`, `A.make()`) is reported as a missing property on valid code, because the class value is typed as its constructor and its static members are not on it.
