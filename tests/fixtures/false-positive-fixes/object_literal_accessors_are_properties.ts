const onlyGetter = {
  get size(): number {
    return 1;
  },
};
const read: number = onlyGetter.size;

const onlySetter = {
  set value(next: string) {},
};
const written: string = onlySetter.value;

const both = {
  set id(next: number) {},
  get id(): number {
    return 1;
  },
};
const id: number = both.id;

const unannotated = {
  get count() {
    return 1;
  },
};
const count = unannotated.count;
