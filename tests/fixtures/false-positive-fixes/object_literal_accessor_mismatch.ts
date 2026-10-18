const source = {
  get size(): number {
    return 1;
  },
};
const wrong: string = source.size;
