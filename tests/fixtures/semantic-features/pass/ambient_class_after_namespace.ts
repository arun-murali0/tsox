declare class C { foo: number }
namespace D { var x = 1; }
declare class D extends C { }
declare namespace G { var y: number }
class G {}
namespace H { type T = number }
class H {}
export {};
