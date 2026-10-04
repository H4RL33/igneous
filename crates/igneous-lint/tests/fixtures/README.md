# Rule fixtures

Each `<rule-id>.json` holds before/after cases for one rule: a list of objects with `name`, `before`, `after`, optional `options` and `source`. `tests/fixtures.rs` runs them.

## Where they come from

The cases are ported from [obsidian-linter](https://github.com/platers/obsidian-linter) (commit `e07b4994503abfa49be12833fcd82cbabd9e9618`, 2026-10-03):

- `source: "example"`: the rule's own documented examples (`exampleBuilders` in `src/rules/<rule>.ts`);
- `source: "test"`: its test cases (`__tests__/<rule>.test.ts`).

They were extracted mechanically, evaluating each case's string and template literals (with `ts-dedent`) and its options. A few options need translating:

- `{"$time": "…"}` is a `moment(…)` time (in the case's `currentTime`), as ISO 8601.
- `fileCreatedTime`, `fileModifiedTime`, `currentTime` and `alreadyModified` are what obsidian-linter passes to `yaml-timestamp` from outside the rule; the test harness turns them into the run's file times, clock and "already modified" flag.

Cases generated in loops or depending on Moment locales weren't extracted. Some cases are known to differ in Igneous; `tests/fixtures.rs` lists them with the reason.

## Licence

obsidian-linter is distributed under the MIT licence:

```
MIT License

Copyright (c) 2021-2022 Victor Tao

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
