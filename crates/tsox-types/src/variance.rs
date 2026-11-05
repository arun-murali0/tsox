// Variance of a generic declaration's type parameters, and relating two applications of the
// same declaration through their arguments.
//
// Problem: `Box<A>` against `Box<B>` was related by expanding both bodies and comparing
// them member by member. That is slow for large declarations, and for a declaration that
// applies itself to a changed argument (`apply<B>(f: Vec2_T<(a: A) => B>): Vec2_T<B>`) each
// expansion produces a new application to expand.
// Picked: read how the declaration uses each parameter, once, and compare the arguments
// by that. tsc gets the same digest by comparing instantiations over marker types
// (relater.go getVariancesWorker); here the declaration body is walked with a polarity
// instead, because relations take a shared `&TypeArena` and cannot allocate marker types.
// The two agree where the body is plain: properties read covariantly, the parameters of a
// function-typed property flip, the parameters of a method are bivariant (the rule
// subtyping::property_is_subtype applies).
// Cost: a body this walk cannot read exactly (a conditional type, a declaration that is
// not finished, mutual recursion between declarations) gives no answer, and the relation
// stays structural for it. An answer is never guessed.

use std::rc::Rc;

use crate::arena::{TypeArena, TypeId};
use crate::fxhash::FxHashMap;
use crate::types::{FunctionType, Type, TypeParameterId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variance {
    /// The parameter is not used: any argument relates to any other.
    Independent,
    Covariant,
    Contravariant,
    /// Used only where either direction is accepted (the parameter of a method).
    Bivariant,
    Invariant,
}

// The direction a position asks of its type: `Co` wants source <: target, `Contra` the
// reverse, `Bi` either, `Inv` both.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Pol {
    Co,
    Contra,
    Bi,
    Inv,
}

impl Pol {
    fn flip(self) -> Self {
        match self {
            Self::Co => Self::Contra,
            Self::Contra => Self::Co,
            other => other,
        }
    }

    // The direction an argument sits in when this position holds a parameter of a
    // declaration used as `variance`. `None` is a parameter that is not used at all.
    fn through(self, variance: Variance) -> Option<Self> {
        match variance {
            Variance::Independent => None,
            Variance::Covariant => Some(self),
            Variance::Contravariant => Some(self.flip()),
            Variance::Bivariant => Some(if self == Self::Inv {
                Self::Inv
            } else {
                Self::Bi
            }),
            Variance::Invariant => Some(Self::Inv),
        }
    }
}

#[derive(Default)]
struct Occurrences {
    co: bool,
    contra: bool,
    bi: bool,
}

impl Occurrences {
    fn record(&mut self, pol: Pol) {
        match pol {
            Pol::Co => self.co = true,
            Pol::Contra => self.contra = true,
            Pol::Bi => self.bi = true,
            Pol::Inv => {
                self.co = true;
                self.contra = true;
            }
        }
    }

    fn variance(&self) -> Variance {
        match (self.co, self.contra, self.bi) {
            (true, true, _) => Variance::Invariant,
            (true, false, _) => Variance::Covariant,
            (false, true, _) => Variance::Contravariant,
            (false, false, true) => Variance::Bivariant,
            (false, false, false) => Variance::Independent,
        }
    }
}

struct Walk<'a> {
    arena: &'a TypeArena,
    base: TypeId,
    target: TypeParameterId,
    params: &'a [TypeParameterId],
    // What the declaration's own parameters were found to be on the previous round. A
    // self-application with a changed argument reads them, which is a fixed point.
    approx: &'a [Variance],
    seen: FxHashMap<(TypeId, Pol), ()>,
    found: Occurrences,
    unreliable: bool,
}

impl Walk<'_> {
    fn walk(&mut self, id: TypeId, pol: Pol) {
        if self.seen.insert((id, pol), ()).is_some() {
            return;
        }
        let arena = self.arena;
        match arena.raw(id) {
            Type::Ref(_) => {
                // The declaration naming itself with its own parameters adds nothing.
                if id == self.base {
                    return;
                }
                match arena.declared_body(id) {
                    Some(body) => self.walk(body, pol),
                    None => self.unreliable = true,
                }
            }
            Type::Named(_, inner) => self.walk(*inner, pol),
            Type::App(_, args, _) => self.walk_app(id, args, pol),
            Type::GenericParameter(parameter, _, _) => {
                if *parameter == self.target {
                    self.found.record(pol);
                }
            }
            Type::Array(element) => self.walk(*element, pol),
            Type::Tuple(elements) => {
                for element in elements {
                    self.walk(element.type_id, pol);
                }
            }
            Type::Union(members) | Type::Intersection(members) => {
                for member in members {
                    self.walk(*member, pol);
                }
            }
            Type::Object(object) => {
                for property in object.properties.iter() {
                    match arena.raw(property.type_id) {
                        Type::Function(function) if property.is_method => {
                            self.walk_function(function, pol, true);
                        }
                        _ => self.walk(property.type_id, pol),
                    }
                }
            }
            Type::Function(function) => self.walk_function(function, pol, false),
            // A conditional type reads its parameters in ways this walk does not model.
            Type::Conditional(_)
            | Type::Mapped(_)
            | Type::Keyof(_)
            | Type::IndexedAccess(..)
            | Type::Template(_)
            | Type::Variadic(_)
            | Type::StringMapping(..) => self.unreliable = true,
            _ => {}
        }
    }

    fn walk_function(&mut self, function: &FunctionType, pol: Pol, method: bool) {
        for param in &function.params {
            let position = if method {
                if pol == Pol::Inv { Pol::Inv } else { Pol::Bi }
            } else {
                pol.flip()
            };
            self.walk(param.type_id, position);
        }
        self.walk(function.return_type, pol);
    }

    fn walk_app(&mut self, id: TypeId, args: &[TypeId], pol: Pol) {
        let arena = self.arena;
        match arena.app_base_of(id) {
            Some(base) if base == self.base => {
                for (index, &arg) in args.iter().enumerate() {
                    if let Type::GenericParameter(parameter, _, _) = arena.raw(arg)
                        && self.params.get(index) == Some(parameter)
                    {
                        continue;
                    }
                    let Some(&declared) = self.approx.get(index) else {
                        self.unreliable = true;
                        continue;
                    };
                    if let Some(position) = pol.through(declared) {
                        self.walk(arg, position);
                    }
                }
            }
            Some(base) => match variances_of(arena, base) {
                Some(variances) if variances.len() == args.len() => {
                    for (&arg, &declared) in args.iter().zip(variances.iter()) {
                        if let Some(position) = pol.through(declared) {
                            self.walk(arg, position);
                        }
                    }
                }
                _ => self.unreliable = true,
            },
            None => match arena.promise_value_type(id) {
                Some(value) => self.walk(value, pol),
                None => self.unreliable = true,
            },
        }
    }
}

const MAX_ROUNDS: usize = 6;

/// How each type parameter of the declaration `base` is used, in declaration order, or
/// `None` when that cannot be read reliably.
pub(crate) fn variances_of(arena: &TypeArena, base: TypeId) -> Option<Rc<[Variance]>> {
    if let Some(cached) = arena.variance_cached(base) {
        return cached;
    }
    let params = arena.decl_params(base)?.to_vec();
    // A declaration still being resolved has no body to read, and an answer taken from
    // it now would be wrong later, so nothing is stored.
    let body = match arena.raw(base) {
        Type::Ref(_) => arena.declared_body(base)?,
        _ => base,
    };
    // Another declaration asked for this one while it is being worked out: mutual
    // recursion, which is not solved here.
    if !arena.variance_enter(base) {
        return None;
    }
    let result = compute(arena, base, body, &params);
    arena.variance_leave(base);
    arena.variance_store(base, result.clone());
    result
}

fn compute(
    arena: &TypeArena,
    base: TypeId,
    body: TypeId,
    params: &[TypeParameterId],
) -> Option<Rc<[Variance]>> {
    let mut approx = vec![Variance::Independent; params.len()];
    for _ in 0..MAX_ROUNDS {
        let mut next = Vec::with_capacity(params.len());
        for &target in params {
            let mut walk = Walk {
                arena,
                base,
                target,
                params,
                approx: &approx,
                seen: FxHashMap::default(),
                found: Occurrences::default(),
                unreliable: false,
            };
            walk.walk(body, Pol::Co);
            if walk.unreliable {
                return None;
            }
            next.push(walk.found.variance());
        }
        if next == approx {
            return Some(Rc::from(next));
        }
        approx = next;
    }
    None
}

// The declaration and arguments of `id` when it is an application, through any alias
// that only names it.
fn application_of(arena: &TypeArena, mut id: TypeId) -> Option<(TypeId, &[TypeId])> {
    loop {
        match arena.raw(id) {
            Type::Named(_, inner) => id = *inner,
            Type::App(_, args, _) => return arena.app_base_of(id).map(|base| (base, &args[..])),
            _ => return None,
        }
    }
}

/// Relates two applications of the same generic declaration through their arguments.
/// `None` means variance does not decide this pair (different declarations, not
/// applications, or a declaration whose use cannot be read) and the caller compares the
/// bodies as before.
pub(crate) fn relate_by_variance(
    arena: &TypeArena,
    sub: TypeId,
    sup: TypeId,
    related: &mut dyn FnMut(TypeId, TypeId) -> bool,
) -> Option<bool> {
    let (sub_base, sub_args) = application_of(arena, sub)?;
    let (sup_base, sup_args) = application_of(arena, sup)?;
    if sub_base != sup_base || sub_args.len() != sup_args.len() {
        return None;
    }
    let variances = variances_of(arena, sub_base)?;
    if variances.len() != sub_args.len() {
        return None;
    }
    for (index, &declared) in variances.iter().enumerate() {
        let (from, to) = (sub_args[index], sup_args[index]);
        let ok = match declared {
            Variance::Independent => true,
            Variance::Covariant => related(from, to),
            Variance::Contravariant => related(to, from),
            Variance::Bivariant => related(to, from) || related(from, to),
            Variance::Invariant => related(from, to) && related(to, from),
        };
        if !ok {
            return Some(false);
        }
    }
    Some(true)
}
