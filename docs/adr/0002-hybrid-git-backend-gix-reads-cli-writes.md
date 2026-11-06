# Git backend lai: gix cho thao tác đọc, git CLI cho thao tác ghi và mạng

tafit đọc dữ liệu repo (lịch sử, refs, config, remote, status, diff) qua `gix` để đạt hiệu năng cao, còn mọi thao tác ghi và mạng (commit, checkout, stash, rebase, cherry-pick, merge, push, fetch, pull, ký và kiểm tra chữ ký) đều gọi git CLI của hệ thống. Lý do: gix chưa hỗ trợ push, ký commit, lưu merge state hay sửa submodule, và tự làm lại hooks, credential helper, LFS, git-annex sẽ dễ lệch hành vi so với git thật. Hệ quả là app yêu cầu người dùng cài git ≥ 2.53; nếu cũ hơn thì hiện thông báo cập nhật chứ không chặn.

Ngoại lệ có chủ đích: hunk/patch (diff của working tree, commit, khoảng commit) lấy bằng cách parse output của `git diff`/`git show`, vì gix chưa sinh được patch. Các lệnh này luôn được gọi với config cố định (`--no-ext-diff --no-color`, prefix `a/` `b/`, `core.quotepath=false`, `diff.suppressBlankEmpty=false`, và với working tree thêm `--no-optional-locks`, `diff.autoRefreshIndex=false`) để config của người dùng không làm sai output.

## Các phương án đã cân nhắc

- **Chỉ dùng gix**: nhanh và tự chứa, nhưng thiếu push/ký commit và phải tự làm lại hooks, credential helper.
- **Chỉ dùng git CLI**: đúng hành vi nhất, nhưng quá chậm khi đọc lịch sử repo cỡ Chromium/Linux.
- **libgit2 (`git2`)**: là thư viện C, không có lợi thế hiệu năng so với gix và vẫn cần CLI cho LFS/annex.
