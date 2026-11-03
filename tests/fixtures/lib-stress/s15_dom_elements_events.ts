// lib.dom: element lookup overloads, event maps, common APIs.
const div: HTMLDivElement = document.createElement("div");
const input: HTMLInputElement | null = document.querySelector("input");
const anyEl: Element | null = document.querySelector(".x");
const typed = document.querySelector<HTMLCanvasElement>("canvas");
const all: NodeListOf<HTMLAnchorElement> = document.querySelectorAll("a");
const byId: HTMLElement | null = document.getElementById("id");
const v: string = input?.value ?? "";
div.addEventListener("click", (e: MouseEvent) => { const x: number = e.clientX; });
window.addEventListener("keydown", e => { const k: string = e.key; });
document.addEventListener("DOMContentLoaded", () => {});
div.classList.add("a");
div.style.color = "red";
div.setAttribute("id", "x");
const ctx: CanvasRenderingContext2D | null = typed ? typed.getContext("2d") : null;
const rect: DOMRect = div.getBoundingClientRect();
const bad1: string = div.clientWidth; // error 2322
div.addEventListener("click", (e: KeyboardEvent) => {}); // error 2769
const bad2: HTMLInputElement = document.createElement("div"); // error 2740
export {};
