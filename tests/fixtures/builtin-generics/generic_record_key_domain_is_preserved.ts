type Dictionary<K extends string, V> = Record<K, V>;

function readWithinDomain<K extends string, V>(
    record: Record<K, V>,
    key: K,
): V {
    return record[key];
}

function readThroughAlias<K extends string, V>(
    record: Dictionary<K, V>,
    key: K,
): V {
    return record[key];
}

function readThroughConstraint<K extends string, V, T extends Record<K, V>>(
    record: T,
    key: K,
): V {
    return record[key];
}

function readWithWideKey<K extends string, V>(
    record: Record<K, V>,
    key: string,
): V {
    return record[key];
}

function readWithForeignLiteral<K extends string, V>(
    record: Record<K, V>,
): V {
    return record["not-in-domain"];
}

function readWithForeignDotName<K extends string, V>(
    record: Record<K, V>,
): V {
    return record.notInDomain;
}

function rejectForeignLiteralKey<K extends string, V>(value: V): void {
    const invalid: Record<K, V> = { notInDomain: value };
}

type Keys<K extends string> = keyof Record<K, number>;
type IndexedValue<K extends string, V> = Record<K, V>[K];

function preserveKeyof<K extends string>(key: Keys<K>): K {
    return key;
}

function preserveIndexedValue<K extends string, V>(value: IndexedValue<K, V>): V {
    return value;
}
