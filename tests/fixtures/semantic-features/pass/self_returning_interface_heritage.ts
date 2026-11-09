interface Q<T> {
  m0<U>(f: (x: T) => U): Q<U>;
  p0: Q<T>;
  m1<U>(f: (x: T) => U): Q<U>;
  p1: Q<T>;
  m2<U>(f: (x: T) => U): Q<U>;
  p2: Q<T>;
  m3<U>(f: (x: T) => U): Q<U>;
  p3: Q<T>;
  m4<U>(f: (x: T) => U): Q<U>;
  p4: Q<T>;
  m5<U>(f: (x: T) => U): Q<U>;
  p5: Q<T>;
  m6<U>(f: (x: T) => U): Q<U>;
  p6: Q<T>;
  m7<U>(f: (x: T) => U): Q<U>;
  p7: Q<T>;
  m8<U>(f: (x: T) => U): Q<U>;
  p8: Q<T>;
  m9<U>(f: (x: T) => U): Q<U>;
  p9: Q<T>;
  m10<U>(f: (x: T) => U): Q<U>;
  p10: Q<T>;
  m11<U>(f: (x: T) => U): Q<U>;
  p11: Q<T>;
  m12<U>(f: (x: T) => U): Q<U>;
  p12: Q<T>;
  m13<U>(f: (x: T) => U): Q<U>;
  p13: Q<T>;
}
interface R<T> extends Q<T> { z: T }
export declare const j: R<number>;
