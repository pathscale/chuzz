# Changelog

## Unreleased

## 0.1.44

- Inspect completed page GPUBuffer writes with original usage, submit serials and typed buffer diagnostics; require blitz-control-protocol ^0.7.1.
- Add the compute and indirect-render buffer proof page with positive, NaN, skipped-compute and frozen-output suites.

## 0.1.43

- Require ps-blitz ^0.6 and show a shared TLS refusal page with typed diagnostics; remove the RSA fallback banner and explainer.

## 0.1.42

- Require blitz-control-protocol ^0.7.0 for inspection protocol 2 and the headless Evaluate response.
- Require izumo ^0.6.0 so GUI inspection uses protocol 2 as well.
- Install ps-qa ^0.9.0 in the headless-host action, CI, and release workflow.
