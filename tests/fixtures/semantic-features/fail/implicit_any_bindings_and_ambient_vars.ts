function f({ x }) {} // error TS7031
function g([z]) {} // error TS7031
function ok({ a = 1 }, [b] = [2]) {}
declare var dv; // error TS7005
export var ev; // error TS7005
