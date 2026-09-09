# test fixtures

個別の挙動を確かめるための fixture。

- suffix 無し / `.with-organization` … octokit の
  [payload-examples](https://github.com/octokit/webhooks/tree/master/payload-examples/api.github.com) 由来
- `.derived.json` … それを手で加工したもの (本文なし、team mention、
  assignee 2 人など、examples に無い形を作るため)

payload を網羅しているかはここでは見ていない。octokit の schema を実行時に
取ってきて `src/github/mod.rs` で確かめている。
