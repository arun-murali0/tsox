// Regression tests for inputs that used to abort the whole process, either with a
// stack overflow or by running out of memory. A crash cannot be caught inside a
// test, so each of these only has to come back at all; the assertions then pin
// down what the checker is allowed to say about the input.
//
// All of them were found by running TypeScript's own test suite (v6.0.3) through
// tsox: 8 of its 10,091 single-file cases aborted before these guards existed.
use tsox::{Diagnostic, TypeChecker};

fn check(source: &str, file_name: &str) -> Vec<Diagnostic> {
    TypeChecker::new()
        .check_source(source, file_name)
        .map(|checked| checked.diagnostics)
        .unwrap_or_default()
}

// Problem: resolving T's constraint `C` needed C's defaults, which pushed C's type
// parameters again and resolved T's constraint again, forever. See
// TypeNamespace::resolving_constraints.
#[test]
fn constraint_naming_its_own_declaration_does_not_recurse() {
    let diagnostics = check("class C1<T extends C1 = any> {}\n", "self_constraint.ts");
    assert!(
        diagnostics.is_empty(),
        "valid code must not gain errors: {diagnostics:?}"
    );
}

// Problem: a default written as its own declaration (`T = I`) asked for I's
// defaults, which asked for T's default again, on an interface, a class and a type
// alias alike. See TypeNamespace::resolving_defaults.
#[test]
fn default_naming_its_own_declaration_does_not_recurse() {
    // The declaration alone does not recurse: the default is only read when the type is
    // used, so each case uses the type as an annotation or builds an instance.
    for (name, source) in [
        (
            "self_default_interface.ts",
            "interface I<T = I> {}\nlet x: I;\n",
        ),
        (
            "self_default_alias.ts",
            "type A<T = A> = { x: T };\nlet x: A;\n",
        ),
        (
            "self_default_class.ts",
            "class K<T = K> {}\nlet x: K;\nlet y = new K();\n",
        ),
        (
            "self_default_both.ts",
            "interface I<T extends I = I> {}\nlet x: I;\n",
        ),
    ] {
        // Whether this should be reported (tsc says TS2716) is a separate question;
        // here it only has to finish.
        let _ = check(source, name);
    }
}

// Problem: a generic class with two members that point back at it printed two
// sub-types per level down to MAX_DEPTH 32, which is 2^32 members, and one error
// message asked for a 4 GB string. See MAX_OUTPUT in type_display.rs.
#[test]
fn error_message_for_a_branching_generic_class_stays_bounded() {
    let cases = [
        "class G<T> { a!: G<T>; b!: G<T>; get(): T { return null as any } }\nlet y: number = new G<string>();\n",
        "class Cell<K, V> { inv!: Cell<V, K>; other!: Cell<V, K>; }\nlet z: number = new Cell<string, number>();\n",
        "class Tree<T> { left!: Tree<T>; right!: Tree<T>; v!: T }\nlet n: string = new Tree<number>();\n",
    ];
    for (index, source) in cases.iter().enumerate() {
        let diagnostics = check(source, &format!("branching_{index}.ts"));
        assert!(
            !diagnostics.is_empty(),
            "the mismatch must still be reported: {source}"
        );
        for diagnostic in &diagnostics {
            // MAX_OUTPUT is 2048; the rest of the message wraps the printed type.
            assert!(
                diagnostic.message.len() < 4096,
                "message not bounded ({} bytes): {source}",
                diagnostic.message.len()
            );
        }
    }
}

// The cap must not touch ordinary messages.
#[test]
fn short_messages_are_not_truncated() {
    let diagnostics = check("let n: number = \"text\";\n", "short_message.ts");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        !diagnostics[0].message.ends_with("..."),
        "{}",
        diagnostics[0].message
    );
}

// Problem: a project run aborted with a stack overflow on one very long chain of
// `+` operators, because rayon's worker threads had the small default stack while
// the same file passed on the main thread. Two files are written so the run has to
// use the workers. See WORKER_STACK_BYTES in module_resolution/check.rs.
#[cfg(feature = "module-resolution")]
#[test]
fn very_long_operator_chain_does_not_overflow_a_worker_stack() {
    let dir = std::env::temp_dir().join(format!("tsox_deep_chain_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("tsconfig.json"), "{}").unwrap();

    // A left-nested tree 4,000 levels deep; the file that failed was about 1,500.
    let chain = vec!["1"; 4000].join(" + ");
    std::fs::write(
        dir.join("deep.ts"),
        format!("export const total = {chain};\n"),
    )
    .unwrap();
    std::fs::write(dir.join("other.ts"), "export const n: number = 1;\n").unwrap();

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_tsox"))
        .arg("--project")
        .arg(dir.join("tsconfig.json"))
        // The workaround this replaces; it must not be needed any more.
        .env_remove("RUST_MIN_STACK")
        .output()
        .unwrap()
        .status;
    let _ = std::fs::remove_dir_all(&dir);

    // A stack overflow is a signal, so there is no exit code at all.
    assert!(
        status.code().is_some_and(|code| code <= 2),
        "the run must finish normally, got {status:?}"
    );
}

// Problem: a generic signature that leads back to itself made the free-parameter walk run
// forever. `Chain<S>` and its `then` method are two types that lead to each other, and the
// walk pushed `then`'s own binder S onto a list on every lap, so a "seen this type under
// these binders" key never repeated (found with gdb: two type ids alternating, the binder
// list growing by one per lap). TypeScript's promiseChaining test hung on it. See
// free_generic_params in semantic/generics.rs.
#[test]
fn self_referential_generic_signature_terminates() {
    let source = "\
class Chain<T> {
    constructor(public value: T) {}
    then<S>(cb: (x: T) => S): Chain<S> {
        const result = cb(this.value);
        const z = this.then(x => result).then(x => \"abc\").then(x => x.length);
        return new Chain(result);
    }
}
declare function unwrap<T>(c: Chain<T>): T;
declare const c: Chain<number>;
const r: string = unwrap(c);
";
    let diagnostics = check(source, "self_referential_signature.ts");
    // The call returns a number, so assigning it to a string must still be reported.
    assert!(
        diagnostics.iter().any(|d| d
            .message
            .contains("Type 'number' is not assignable to type 'string'")),
        "the mismatch must still be reported: {diagnostics:?}"
    );
}

// A generic function's own parameter is bound inside it, so a type holding only such a
// function is not itself generic, while a reference to an enclosing parameter is still free.
#[test]
fn nested_generic_function_binds_its_own_parameter_only() {
    let diagnostics = check(
        "declare function outer<T>(x: T, f: <U>(u: U, t: T) => U): T;\n\
         const r: string = outer(1, (u, t) => u);\n",
        "nested_binder.ts",
    );
    // outer infers T = number from the first argument, so the string annotation is an error.
    assert!(
        diagnostics.iter().any(|d| d
            .message
            .contains("Type 'number' is not assignable to type 'string'")),
        "{diagnostics:?}"
    );
}

// Problem: asking whether a type mentions a type parameter walked every path through
// classes that refer to each other, and a node cut short by a cycle was never
// remembered, so the work grew with the number of paths: 2^n and worse. TypeScript's
// resolvingClassDeclarationWhenInBaseTypeResolution (about 50 generic classes across
// namespaces) never finished. Inference over the same types had the same shape: it
// forgot a pair as soon as its match finished, so each route to it matched it again.
// Here 24 plain classes each point at the next 5, and one call uses the first. See
// free_generic_params and SeenPairs in semantic/generics.rs; both have to be fixed
// for this to finish. The check runs on its own thread so a regression fails this
// test after a timeout instead of freezing the whole suite.
#[test]
fn classes_that_refer_to_each_other_do_not_blow_up_the_parameter_scan() {
    const CLASSES: usize = 24;
    let mut source = String::new();
    for i in 0..CLASSES {
        source.push_str(&format!("class C{i} {{\n"));
        for step in 1..=5 {
            source.push_str(&format!("  p{step}!: C{};\n", (i + step) % CLASSES));
        }
        source.push_str("}\n");
    }
    source.push_str("declare function use(c: C0): void;\ndeclare const c0: C0;\nuse(c0);\n");

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let diagnostics = check(&source, "mutually_referring_classes.ts");
        let _ = sender.send(diagnostics);
    });
    let diagnostics = receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the parameter scan did not finish: its cost is growing with the number of paths");
    assert!(
        diagnostics.is_empty(),
        "valid code must not gain errors: {diagnostics:?}"
    );
}
