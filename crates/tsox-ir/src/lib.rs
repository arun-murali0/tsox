// Problem: a native or WASM back end needs a typed form of the program, and TypeScript's own
// types are not sound at run time (`any`, assertions, structural subtyping, excess properties),
// so lowering straight from the checker's types would trust things nobody checked.
// Picked: a small statically typed IR whose every value records whether its type is trusted
// (`tsox_model::Trust`). It starts as the subset that can be typed soundly; anything outside it
// lowers to `Ty::Dynamic`, which a back end must treat as unchecked.
// Cost: this is a skeleton. Nothing lowers into it yet, and its instruction set is deliberately
// tiny. It exists so the crate boundary and its layer are fixed before the work starts: it may
// read the semantic model and nothing from the checker.

use tsox_model::Trust;

/// An IR type. `Dynamic` is the escape hatch for what the static subset cannot type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    Unit,
    Bool,
    /// A 64-bit float, the only TypeScript `number`.
    F64,
    Str,
    Dynamic,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub enum Inst {
    ConstBool(bool),
    ConstF64(f64),
    ConstStr(Box<str>),
    Add(ValueId, ValueId),
    /// Checks a `Dynamic` value against a type and yields it at that type.
    Guard(ValueId, Ty),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Terminator {
    Return(Option<ValueId>),
    Jump(BlockId),
    Branch {
        cond: ValueId,
        then_block: BlockId,
        else_block: BlockId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    pub ty: Ty,
    pub trust: Trust,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub insts: Vec<(ValueId, Inst)>,
    pub terminator: Terminator,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: Box<str>,
    pub params: Vec<ValueId>,
    pub values: Vec<Value>,
    pub blocks: Vec<Block>,
    pub ret: Ty,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Module {
    pub functions: Vec<Function>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_function_can_be_built_and_compared() {
        let function = Function {
            name: "id".into(),
            params: vec![],
            values: vec![Value {
                ty: Ty::Bool,
                trust: Trust::Trusted,
            }],
            blocks: vec![Block {
                insts: vec![(ValueId(0), Inst::ConstBool(true))],
                terminator: Terminator::Return(Some(ValueId(0))),
            }],
            ret: Ty::Bool,
        };
        let module = Module {
            functions: vec![function.clone()],
        };
        assert_eq!(module.functions[0], function);
    }
}
