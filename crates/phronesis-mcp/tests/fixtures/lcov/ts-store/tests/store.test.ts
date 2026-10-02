import { expect, it } from "vitest";
import { Store } from "../src/store";

it("Store loads", () => {
  expect(new Store().load()).toBe("value");
});