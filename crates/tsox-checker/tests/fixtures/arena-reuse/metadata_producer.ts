enum Status {
  Active = "active",
  Disabled = "disabled",
}

type Conditional<T> = T extends string ? string : number;
type Box<T> = { value: T };

type First = Conditional<string>;
type StatusBox = Box<Status>;
const first: First = "ready";
const statusBox: StatusBox = { value: Status.Active };
