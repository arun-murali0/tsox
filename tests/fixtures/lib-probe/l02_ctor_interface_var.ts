// ES5 pattern: `declare var Array: ArrayConstructor` (call + construct signatures + static).
interface Bar { id: number; }
interface BarConstructor {
    new (v?: any): Bar;
    (v?: any): string;
    readonly prototype: Bar;
    isBar(a: any): a is Bar;
}
declare var Bar: BarConstructor;
const b = new Bar();
const id: number = b.id;
const s: string = Bar(1);
const bad: number = Bar(1); // error 2322
export {};
