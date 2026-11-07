// Who may reach a class member (TS2341, TS2445) and which members may not be written (TS2540).

use oxc_span::Span;

use super::context::CheckContext;
use crate::arena::TypeId;
use crate::diagnostic_messages::messages;
use crate::types::Access;

// A read or write of `object.name`, where `object` is `object_type`. A `private` member is
// reachable only inside the body of the class that declared it; a `protected` one also inside a
// class that inherited it (its instance type still names the same declaring class).
pub(super) fn check_member_accessibility(
    ctx: &mut CheckContext<'_, '_>,
    object_type: TypeId,
    name: &str,
    span: Span,
) {
    let Some(modifiers) = ctx.arena.property_modifiers(object_type, name) else {
        return;
    };
    if modifiers.access == Access::Public {
        return;
    }
    let Some((declaring, class_name)) = &modifiers.declaring_class else {
        return;
    };
    if ctx.current_class_ids.contains(declaring) {
        return;
    }
    if modifiers.access == Access::Protected {
        let inherited = ctx.current_class_instance.is_some_and(|instance| {
            ctx.arena
                .property_modifiers(instance, name)
                .and_then(|own| own.declaring_class)
                .is_some_and(|(id, _)| id == *declaring)
        });
        if inherited {
            return;
        }
        ctx.error(messages::protected_property_access(name, class_name), span);
    } else {
        ctx.error(messages::private_property_access(name, class_name), span);
    }
}

// TS2540: a write to a `readonly` property. Inside a constructor, `this.x = ...` is how a
// `readonly` field gets its value.
pub(super) fn check_readonly_write(
    ctx: &mut CheckContext<'_, '_>,
    object_type: TypeId,
    object_is_this: bool,
    name: &str,
    span: Span,
) -> bool {
    let Some(modifiers) = ctx.arena.property_modifiers(object_type, name) else {
        return false;
    };
    if modifiers.readonly && !(object_is_this && ctx.in_constructor) {
        ctx.error(messages::readonly_property_assignment(name), span);
        return true;
    }
    false
}
