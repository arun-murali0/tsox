// Every level has the same four property names. The leaf differs only in the type of
// `a`; structural comparison must continue far enough to discover string is not number.
type SourceLeaf = { a: string; b: string; c: string; d: string };
type TargetLeaf = { a: number; b: string; c: string; d: string };

type SourceLevel2 = { a: SourceLeaf; b: SourceLeaf; c: SourceLeaf; d: SourceLeaf };
type TargetLevel2 = { a: TargetLeaf; b: TargetLeaf; c: TargetLeaf; d: TargetLeaf };

type SourceLevel1 = { a: SourceLevel2; b: SourceLevel2; c: SourceLevel2; d: SourceLevel2 };
type TargetLevel1 = { a: TargetLevel2; b: TargetLevel2; c: TargetLevel2; d: TargetLevel2 };

type SourceRoot = { a: SourceLevel1; b: SourceLevel1; c: SourceLevel1; d: SourceLevel1 };
type TargetRoot = { a: TargetLevel1; b: TargetLevel1; c: TargetLevel1; d: TargetLevel1 };

declare const source: SourceRoot;
const invalid: TargetRoot = source;
