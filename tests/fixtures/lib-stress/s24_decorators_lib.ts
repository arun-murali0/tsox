// lib.decorators: standard decorator context types.
function logged<T extends (...a: any[]) => any>(target: T, ctx: ClassMethodDecoratorContext): T {
  const name: string | symbol = ctx.name;
  return target;
}
function sealed(target: Function, ctx: ClassDecoratorContext) { const k: "class" = ctx.kind; }
function field(_: undefined, ctx: ClassFieldDecoratorContext) { return (v: number) => v; }
@sealed
class C {
  @field x = 1;
  @logged m(): number { return 1; }
}
const k: string = new C().m().toString();
export {};
