;;; auximap.el --- Fast IMAP email reader powered by auximap.exe -*- lexical-binding: t; -*-

;; Author: auximap contributors
;; Keywords: mail, imap, preview, dired
;; Version: 0.3.1
;;
;;; Commentary:
;;
;; auximap.el provides a split-window email reader for Emacs
;; that combines Dired-like and PPx-like marking and batch operations
;; with `auximap.exe` as its lightweight, ultra-fast backend.
;;
;; Features:
;; - Two-pane view: Email list (top) and Body preview (bottom)
;; - Blazing-fast browsing: Move cursor (`n`/`p` or arrow keys) to preview instantly.
;; - PPx & Dired style marking:
;;   - `SPC`: Mark email with `*` and move down (PPx style)
;;   - `d`: Mark email for deletion with `D` and move down (Dired style)
;;   - `C-a` / `U`: Unmark all emails (PPx & Dired style)
;;   - `u`: Unmark current email and move down
;;   - `x`: Execute deletion (move marked emails to Trash folder with y/n confirmation)
;; - Batch keyword operations:
;;   - `% m` or `m`: Mark emails matching keyword/regex
;;   - `% d`: Mark emails matching keyword for deletion
;;   - `% u`: Unmark emails matching keyword
;;   - `/`: Filter email list by keyword (press `//` or `g` to clear filter)
;; - In-memory and disk caching: Viewed emails load with zero latency.
;; - Full UTF-8 support with MIME auto-decoding and HTML text extraction.
;; Setup Example (init.el):
;;
;;   ;; 1. Simple setup
;;   (add-to-list 'load-path "~/.emacs.d/lisp")
;;   (require 'auximap)
;;   ;; (setq auximap-default-account "your_account") ; auto-detected from auximap.ini if omitted
;;   (setq auximap-default-folder  "INBOX")          ; defaults to "INBOX"
;;   (global-set-key (kbd "<f9>") 'auximap-toggle)
;;
;;   ;; 2. Using use-package
;;   (use-package auximap
;;     :load-path "~/.emacs.d/lisp"
;;     :bind ("<f9>" . auximap-toggle)
;;     :custom
;;     ;; (auximap-default-account "your_account")
;;     (auximap-default-folder "INBOX"))
;;
;;   ;; Note: auximap.exe is automatically detected from PATH or .emacs.d/../bin/
;;   ;; You can specify manually if needed:
;;   ;; (setq auximap-program "C:/path/to/auximap.exe")
;;
;; Start:
;;   M-x auximap        (or press <f9> if bound to auximap-toggle)

;;; Code:

(require 'cl-lib)
(require 'subr-x)

(defgroup auximap nil
  "Fast IMAP reader using auximap.exe backend."
  :group 'mail
  :prefix "auximap-")

(defcustom auximap-program
  (or (let ((p (expand-file-name "../../bin/auximap.exe" invocation-directory)))
        (when (file-exists-p p) p))
      (let ((p (expand-file-name "../bin/auximap.exe" (or (bound-and-true-p user-emacs-directory) "~/.emacs.d"))))
        (when (file-exists-p p) p))
      (executable-find "auximap.exe")
      (executable-find "auximap")
      "auximap.exe")
  "Path to the auximap.exe executable."
  :type 'file
  :group 'auximap)

(defcustom auximap-default-account nil
  "Default IMAP account name configured in auximap.ini.
If nil, the first account defined in auximap.ini will be used automatically."
  :type '(choice (const :tag "First account in ini" nil) string)
  :group 'auximap)

(defcustom auximap-ini-file nil
  "Explicit path to auximap.ini. If nil, automatically searched near `auximap-program`."
  :type '(choice (const :tag "Auto" nil) file)
  :group 'auximap)

(defcustom auximap-default-folder "INBOX"
  "Default folder to open on startup."
  :type 'string
  :group 'auximap)

(defcustom auximap-cache-dir
  (expand-file-name "auximap-cache" temporary-file-directory)
  "Directory to cache fetched email bodies."
  :type 'directory
  :group 'auximap)

(defcustom auximap-window-split-ratio 0.45
  "Fraction of the frame height allocated to the email list window."
  :type 'float
  :group 'auximap)

(defcustom auximap-auto-preview t
  "When non-nil, automatically preview the email at point on cursor movement."
  :type 'boolean
  :group 'auximap)

;; Faces
(defface auximap-header-face
  '((t :inherit font-lock-keyword-face :weight bold))
  "Face for email headers."
  :group 'auximap)

(defface auximap-date-face
  '((t :inherit font-lock-constant-face))
  "Face for date in list."
  :group 'auximap)

(defface auximap-from-face
  '((t :inherit font-lock-function-name-face))
  "Face for sender in list."
  :group 'auximap)

(defface auximap-subject-face
  '((t :inherit default))
  "Face for subject in list."
  :group 'auximap)

(defface auximap-mark-face
  '((t :foreground "gold" :weight bold))
  "Face for marked entries (*)."
  :group 'auximap)

(defface auximap-delete-face
  '((t :foreground "tomato" :weight bold))
  "Face for deletion-marked entries (D)."
  :group 'auximap)

;; Buffer locals
(defvar-local auximap-current-account nil)
(defvar-local auximap-current-folder nil)
(defvar-local auximap-saved-window-config nil)
(defvar-local auximap-all-entries nil)     ; All parsed entries: list of plists
(defvar-local auximap-current-filter nil)   ; Active filter keyword or nil

;; Internal cache
(defvar auximap--memory-cache (make-hash-table :test 'equal))
(defvar auximap--last-previewed-uid nil)
(defvar auximap--password-cache (make-hash-table :test 'equal))

(defun auximap--get-password (account)
  "Get password for ACCOUNT from memory cache or prompt user with `read-passwd`."
  (let ((cached (gethash account auximap--password-cache)))
    (if (and cached (not (string-empty-p cached)))
        cached
      (let ((entered (read-passwd (format "Password for %s: " account))))
        (if (and entered (not (string-empty-p entered)))
            (progn
              (puthash account entered auximap--password-cache)
              entered)
          nil)))))

(defun auximap-clear-auth ()
  "Clear in-memory password cache (equivalent to PPx *clearauth)."
  (interactive)
  (clrhash auximap--password-cache)
  (message "auximap: 認証キャッシュを消去しました。"))

(defun auximap--call-process (account &rest args)
  "Call `auximap-program` with ARGS for ACCOUNT, injecting --pass if available."
  (let* ((pass (auximap--get-password account))
         (pass-arg (when (and pass (not (string-empty-p pass)))
                     (list (format "--pass=%s" pass))))
         (full-args (append (list auximap-program nil nil nil) args pass-arg))
         (exit-code (apply #'call-process full-args)))
    (when (not (zerop exit-code))
      ;; エラー時は誤ったパスワードの可能性を考慮してキャッシュを消去
      (remhash account auximap--password-cache))
    exit-code))

(defun auximap--get-ini-path ()
  "Find the path to auximap.ini."
  (or (and auximap-ini-file (file-exists-p auximap-ini-file) auximap-ini-file)
      (let* ((prog (or (executable-find auximap-program) auximap-program))
             (prog-dir (and prog (file-name-directory prog)))
             (near-prog (and prog-dir (expand-file-name "auximap.ini" prog-dir))))
        (when (and near-prog (file-exists-p near-prog))
          near-prog))
      (let* ((el-dir (and (or load-file-name buffer-file-name)
                          (file-name-directory (or load-file-name buffer-file-name))))
             (parent-dir (and el-dir (expand-file-name ".." el-dir)))
             (near-el (and parent-dir (expand-file-name "auximap.ini" parent-dir))))
        (when (and near-el (file-exists-p near-el))
          near-el))
      (let ((in-bin (expand-file-name "../../bin/auximap.ini" invocation-directory)))
        (when (file-exists-p in-bin) in-bin))))

(defun auximap--fetch-accounts ()
  "Fetch configured account list from auximap.ini dynamically."
  (let ((accounts nil)
        (ini-path (auximap--get-ini-path)))
    (when (and ini-path (file-exists-p ini-path))
      (with-temp-buffer
        (let ((coding-system-for-read 'utf-8-dos))
          (insert-file-contents ini-path))
        (goto-char (point-min))
        (while (re-search-forward "^\\[\\([^]]+\\)\\]" nil t)
          (let ((sec (match-string 1)))
            (unless (or (member (upcase sec) '("DEFAULT" "VIEW"))
                        (string-prefix-p "RULE:" (upcase sec)))
              (push sec accounts))))))
    (setq accounts (nreverse accounts))
    (or accounts
        (and auximap-current-account (list auximap-current-account)))))

;;;###autoload
(defun auximap (&optional account folder)
  "Start auximap email reader with ACCOUNT and FOLDER."
  (interactive)
  (let* ((accs (auximap--fetch-accounts))
         (acc (or account
                  auximap-default-account
                  (car accs)
                  "INBOX"))
         (fld (or folder auximap-default-folder)))
    (unless (file-exists-p auximap-cache-dir)
      (make-directory auximap-cache-dir t))
    (auximap-open-mailbox acc fld)))

(defun auximap-open-mailbox (account folder)
  "Open email mailbox for ACCOUNT and FOLDER in split-window mode."
  (let* ((buf-name (format "*auximap: %s/%s*" account folder))
         (list-buf (get-buffer-create buf-name))
         (view-buf (get-buffer-create "*auximap-view*"))
         (win-config (current-window-configuration)))

    ;; Setup windows
    (delete-other-windows)
    (switch-to-buffer list-buf)
    (let ((list-win (selected-window))
          (view-win (split-window-below (round (* (window-height) auximap-window-split-ratio)))))
      (set-window-buffer view-win view-buf)
      (with-current-buffer view-buf
        (auximap-view-mode)
        (let ((inhibit-read-only t))
          (erase-buffer)
          (insert "=== auximap: メールを選択するとここに本文が表示されます ===\n")))
      (select-window list-win))

    ;; Setup list buffer
    (with-current-buffer list-buf
      (auximap-mode)
      (setq auximap-current-account account)
      (setq auximap-current-folder folder)
      (setq auximap-saved-window-config win-config)
      (setq auximap-current-filter nil)
      ;; 画面を開くたびにサーバーから一覧を再取得する
      (auximap-reload))))

;;; ----------------------------------------------------------------------------
;;; Mode Definitions & Keymaps
;;; ----------------------------------------------------------------------------

(defvar auximap-mode-map
  (let ((map (make-sparse-keymap)))
    ;; Navigation
    (define-key map (kbd "n") #'auximap-next-email)
    (define-key map (kbd "p") #'auximap-prev-email)
    (define-key map (kbd "j") #'auximap-next-email)
    (define-key map (kbd "k") #'auximap-prev-email)
    (define-key map (kbd "<down>") #'auximap-next-email)
    (define-key map (kbd "<up>") #'auximap-prev-email)
    (define-key map (kbd "C-n") #'auximap-next-email)
    (define-key map (kbd "C-p") #'auximap-prev-email)
    (define-key map (kbd "RET") #'auximap-focus-view)
    (define-key map (kbd "v") #'auximap-preview-current)
    (define-key map (kbd "P") #'auximap-toggle-auto-preview)

    ;; Scrolling view pane
    (define-key map (kbd "<tab>") #'auximap-scroll-view-up)
    (define-key map (kbd "S-<iso-lefttab>") #'auximap-scroll-view-down)
    (define-key map (kbd "S-<tab>") #'auximap-scroll-view-down)

    ;; Marking (PPx & Dired style)
    (define-key map (kbd "SPC") #'auximap-mark-toggle-and-next)
    (define-key map (kbd "d") #'auximap-mark-delete-and-next)
    (define-key map (kbd "u") #'auximap-unmark-and-next)
    (define-key map (kbd "C-a") #'auximap-unmark-all)
    (define-key map (kbd "U") #'auximap-unmark-all)
    (define-key map (kbd "t") #'auximap-toggle-all-marks)

    ;; Batch operations
    (define-key map (kbd "m") #'auximap-mark-by-keyword)
    (define-key map (kbd "% m") #'auximap-mark-by-keyword)
    (define-key map (kbd "% d") #'auximap-mark-delete-by-keyword)
    (define-key map (kbd "% u") #'auximap-unmark-by-keyword)
    (define-key map (kbd "/") #'auximap-filter-by-keyword)

    ;; Execution (Delete marked emails)
    (define-key map (kbd "x") #'auximap-execute-delete)
    (define-key map (kbd "D") #'auximap-execute-delete)

    ;; Management
    (define-key map (kbd "g") #'auximap-refresh)
    (define-key map (kbd "r") #'auximap-refresh)
    (define-key map (kbd "f") #'auximap-change-folder)
    (define-key map (kbd "c") #'auximap-change-folder)
    (define-key map (kbd "a") #'auximap-change-account)
    (define-key map (kbd "q") #'auximap-quit)
    (define-key map (kbd "<f1>") #'auximap-help)
    (define-key map (kbd "<F1>") #'auximap-help)
    (define-key map (kbd "<f9>") #'auximap-toggle)
    (define-key map (kbd "<F9>") #'auximap-toggle)
    (define-key map (kbd "?") #'auximap-help)
    (define-key map (kbd "h") #'auximap-help)
    map)
  "Keymap for `auximap-mode'.")

(define-derived-mode auximap-mode special-mode "auximap"
  "Major mode for browsing IMAP email lists with Dired/PPx markings."
  (setq buffer-read-only t)
  (setq truncate-lines t)
  (hl-line-mode 1))

(defvar auximap-view-mode-map
  (let ((map (make-sparse-keymap)))
    (define-key map (kbd "q") #'auximap-view-quit-to-list)
    (define-key map (kbd "n") #'auximap-view-next)
    (define-key map (kbd "p") #'auximap-view-prev)
    (define-key map (kbd "SPC") #'scroll-up-command)
    (define-key map (kbd "b") #'scroll-down-command)
    (define-key map (kbd "<prior>") #'scroll-down-command)
    (define-key map (kbd "<next>") #'scroll-up-command)
    (define-key map (kbd "<f1>") #'auximap-view-help)
    (define-key map (kbd "<F1>") #'auximap-view-help)
    (define-key map (kbd "<f9>") #'auximap-toggle)
    (define-key map (kbd "<F9>") #'auximap-toggle)
    (define-key map (kbd "?") #'auximap-view-help)
    (define-key map (kbd "h") #'auximap-view-help)
    map)
  "Keymap for `auximap-view-mode'.")

(define-derived-mode auximap-view-mode special-mode "auximap-view"
  "Major mode for viewing email bodies."
  (setq buffer-read-only t)
  (setq-local font-lock-defaults
              '((("^\\(From\\|To\\|Cc\\|Date\\|Subject\\|Attach\\)[ \t]*:" . 'auximap-header-face)
                 ("^--------------------------------------------------------------------------------" . 'font-lock-comment-face)))))

;;; ----------------------------------------------------------------------------
;;; Help & Guide (Hydra / F1)
;;; ----------------------------------------------------------------------------

(require 'hydra nil t)

(when (or (featurep 'hydra) (fboundp 'defhydra))
  (defhydra hydra-auximap-help (:color blue :hint nil)
    "
  === auximap メール一覧 操作ガイド ===  [F1 / F9 / q] 閉じる
  [移動・プレビュー]              [マーク操作 (PPx/Dired)]  [一括・フィルタ]
  n / j / ↓ : 次のメール          SPC   : マークして次へ    m / ％m : キーワードマーク
  p / k / ↑ : 前のメール          d     : 削除マーク        ％d     : キーワード削除マーク
  Tab       : 本文スクロール      u     : マーク解除        ％u     : キーワードマーク解除
  S-Tab     : 本文逆スクロール    C-a/U : 全マーク解除      /       : 絞り込み (空で全件)
  RET       : 本文へフォーカス    t     : マーク全反転      x / D   : マーク一括削除実行
  v         : 本文再読込(強制)
  P         : プレビュー自動ON/OFF
  [管理・更新]
  g / r     : 一覧を再取得        f / c : フォルダ切替
  a         : アカウント切替      q / F9: 終了・閉じる
  ----------------------------------------------------------------------
"
    ("n" auximap-next-email :color blue)
    ("j" auximap-next-email :color blue)
    ("<down>" auximap-next-email :color blue)
    ("p" auximap-prev-email :color blue)
    ("k" auximap-prev-email :color blue)
    ("<up>" auximap-prev-email :color blue)
    ("<tab>" auximap-scroll-view-up :color blue)
    ("S-<tab>" auximap-scroll-view-down :color blue)
    ("RET" auximap-focus-view :color blue)
    ("<return>" auximap-focus-view :color blue)
    ("v" auximap-preview-current :color blue)
    ("P" auximap-toggle-auto-preview :color blue)
    ("SPC" auximap-mark-toggle-and-next :color blue)
    ("d" auximap-mark-delete-and-next :color blue)
    ("u" auximap-unmark-and-next :color blue)
    ("C-a" auximap-unmark-all :color blue)
    ("U" auximap-unmark-all :color blue)
    ("t" auximap-toggle-all-marks :color blue)
    ("m" auximap-mark-by-keyword :color blue)
    ("/" auximap-filter-by-keyword :color blue)
    ("x" auximap-execute-delete :color blue)
    ("D" auximap-execute-delete :color blue)
    ("g" auximap-refresh :color blue)
    ("r" auximap-refresh :color blue)
    ("f" auximap-change-folder :color blue)
    ("c" auximap-change-folder :color blue)
    ("a" auximap-change-account :color blue)
    ("q" nil :color blue)
    ("<escape>" nil :color blue)
    ("<f1>" nil :color blue)
    ("<F1>" nil :color blue)
    ("<f9>" nil :color blue)
    ("<F9>" nil :color blue))

  (defhydra hydra-auximap-view-help (:color blue :hint nil)
    "
  === auximap 本文閲覧 操作ガイド ===  [F1 / F9 / q] 閉じる
  [スクロール・移動]              [ナビゲーション]
  SPC / PgDn : 次ページへスクロール  n : 次のメールへ移動
  b   / PgUp : 前ページへスクロール  p : 前のメールへ移動
  q / F9     : 一覧へ戻る (フォーカス復帰)
  ----------------------------------------------------------------------
"
    ("SPC" scroll-up-command :color blue)
    ("<next>" scroll-up-command :color blue)
    ("b" scroll-down-command :color blue)
    ("<prior>" scroll-down-command :color blue)
    ("n" auximap-view-next :color blue)
    ("p" auximap-view-prev :color blue)
    ("q" auximap-view-quit-to-list :color blue)
    ("<escape>" auximap-view-quit-to-list :color blue)
    ("<f1>" nil :color blue)
    ("<F1>" nil :color blue)
    ("<f9>" nil :color blue)
    ("<F9>" nil :color blue)))

(defun auximap-help ()
  "Show help guide for auximap mode."
  (interactive)
  (if (fboundp 'hydra-auximap-help/body)
      (hydra-auximap-help/body)
    (describe-mode)))

(defun auximap-view-help ()
  "Show help guide for auximap view mode."
  (interactive)
  (if (fboundp 'hydra-auximap-view-help/body)
      (hydra-auximap-view-help/body)
    (describe-mode)))

;;; ----------------------------------------------------------------------------
;;; Core Operations: Fetch & Render
;;; ----------------------------------------------------------------------------

(defun auximap-refresh ()
  "Fetch email list from server and render."
  (interactive)
  (let* ((acc auximap-current-account)
         (fld auximap-current-folder)
         (target (format "%s/%s" acc fld))
         (temp-list-file (expand-file-name (format "list_%s_%s.txt" acc (replace-regexp-in-string "[/\\\\]" "_" fld)) auximap-cache-dir)))
    (message "Fetching email list for %s..." target)
    (let ((inhibit-read-only t))
      (erase-buffer)
      (insert (format "== [%s / %s] 取得中... ==\n" acc fld)))
    (redisplay)

    (let ((exit-code (auximap--call-process acc "list" target temp-list-file)))
      (if (not (zerop exit-code))
          (message "auximap error: failed to fetch list (exit code %d)" exit-code)
        (auximap--load-entries-from-file temp-list-file)
        (auximap--render-buffer)
        (message "Email list updated: %d emails." (length auximap-all-entries))
        (goto-char (point-min))
        (forward-line 2) ; skip header lines
        (when auximap-auto-preview
          (auximap-preview-current))))))

(defalias 'auximap-reload #'auximap-refresh
  "Reload email list from server and render (alias of `auximap-refresh`).")

(defun auximap--load-entries-from-file (list-file)
  "Parse LIST-FILE into `auximap-all-entries`."
  (let ((entries nil))
    (when (file-exists-p list-file)
      (with-temp-buffer
        (let ((coding-system-for-read 'utf-8-dos))
          (insert-file-contents list-file))
        (goto-char (point-min))
        (while (not (eobp))
          (let ((line (string-trim (buffer-substring-no-properties (line-beginning-position) (line-end-position)))))
            (when (and (not (string-empty-p line))
                       (not (string-prefix-p ";" line)))
              (let ((fname (if (string-match "\"\\([^\"]+\\)\"" line)
                               (match-string 1 line)
                             line)))
                (when (string-match "\\[\\([0-9]+\\)\\]\\s-*\\(.*?\\)\\s-*-\\s-*\\(.*\\)\\.eml$" fname)
                  (let ((uid (match-string 1 fname))
                        (subj (match-string 2 fname))
                        (sender (match-string 3 fname)))
                    (push (list :uid uid :subject subj :from sender :raw fname :mark nil) entries))))))
          (forward-line 1))))
    (setq auximap-all-entries (nreverse entries))))

(defun auximap--render-buffer ()
  "Render the list buffer reflecting marks and active filters."
  (let* ((inhibit-read-only t)
         (saved-uid (when-let ((e (auximap-current-entry))) (plist-get e :uid)))
         (displayed-entries (if auximap-current-filter
                                (cl-remove-if-not
                                 (lambda (e)
                                   (let ((kw (downcase auximap-current-filter))
                                         (subj (downcase (plist-get e :subject)))
                                         (from (downcase (plist-get e :from))))
                                     (or (string-match-p (regexp-quote kw) subj)
                                         (string-match-p (regexp-quote kw) from))))
                                 auximap-all-entries)
                              auximap-all-entries))
         (marked-count (cl-count-if (lambda (e) (plist-get e :mark)) auximap-all-entries)))

    (erase-buffer)

    ;; Header line
    (let ((filter-info (if auximap-current-filter (format " [フィルタ: \"%s\"]" auximap-current-filter) ""))
          (mark-info (if (> marked-count 0) (format " (マーク: %d件)" marked-count) "")))
      (insert (propertize (format "== [%s / %s] %d件%s%s ==  "
                                  auximap-current-account auximap-current-folder
                                  (length displayed-entries) filter-info mark-info)
                          'face 'font-lock-comment-face))
      (insert-text-button "[🔄 更新 (r)]"
                          'action (lambda (_) (auximap-reload))
                          'help-echo "クリックまたは 'r' / 'g' でメール一覧を再取得"
                          'follow-link t)
      (insert " ")
      (insert-text-button "[❓ ガイド (F1)]"
                          'action (lambda (_) (auximap-help))
                          'help-echo "クリックまたは 'F1' / '?' で操作ガイドを表示"
                          'follow-link t)
      (insert "\n"))

    ;; Column titles
    (insert (propertize (format "%-2s %-7s | %-30s | %s\n" "M" "UID" "送信者" "件名")
                        'face 'auximap-header-face))
    (insert (propertize (make-string (max 80 (window-width)) ?-) 'face 'font-lock-comment-face) "\n")

    ;; Rows
    (if (null displayed-entries)
        (insert (if auximap-current-filter
                    "  （フィルタに一致するメールはありません。[//] または [g] で解除）\n"
                  (format "  （%s / %s にメールはありません）\n" auximap-current-account auximap-current-folder)))
      (dolist (item displayed-entries)
        (let* ((uid (plist-get item :uid))
               (sender (truncate-string-to-width (plist-get item :from) 30 0 nil "..."))
               (subj (plist-get item :subject))
               (mark (plist-get item :mark))
               (mark-char (cond
                           ((eq mark ?*) "* ")
                           ((eq mark ?D) "D ")
                           (t "  ")))
               (mark-face (cond
                           ((eq mark ?*) 'auximap-mark-face)
                           ((eq mark ?D) 'auximap-delete-face)
                           (t nil)))
               (start (point)))
          (insert (if mark-face (propertize mark-char 'face mark-face) mark-char))
          (insert (format "%-7s | %-30s | %s\n" uid sender subj))
          (add-text-properties start (point)
                               (list 'auximap-entry item
                                     'mouse-face 'highlight)))))

    ;; Restore cursor position
    (goto-char (point-min))
    (forward-line 3)
    (when saved-uid
      (while (and (not (eobp))
                  (let ((e (auximap-current-entry)))
                    (and e (not (equal (plist-get e :uid) saved-uid)))))
        (forward-line 1)))))

(defun auximap-current-entry ()
  "Get the entry plist at point."
  (get-text-property (point) 'auximap-entry))

;;; ----------------------------------------------------------------------------
;;; Marking Commands (PPx & Dired Style)
;;; ----------------------------------------------------------------------------

(defun auximap-mark-toggle-and-next ()
  "Toggle mark `*` on current email and advance to the next line (PPx style)."
  (interactive)
  (when-let ((entry (auximap-current-entry)))
    (let ((current-mark (plist-get entry :mark)))
      (plist-put entry :mark (if (eq current-mark ?*) nil ?*))
      (auximap--render-buffer)
      (auximap-next-email))))

(defun auximap-mark-delete-and-next ()
  "Mark current email with `D` (for deletion) and advance (Dired style)."
  (interactive)
  (when-let ((entry (auximap-current-entry)))
    (plist-put entry :mark ?D)
    (auximap--render-buffer)
    (auximap-next-email)))

(defun auximap-unmark-and-next ()
  "Remove mark on current email and advance to the next line."
  (interactive)
  (when-let ((entry (auximap-current-entry)))
    (plist-put entry :mark nil)
    (auximap--render-buffer)
    (auximap-next-email)))

(defun auximap-unmark-all ()
  "Unmark all emails (PPx Ctrl+A / Dired U style)."
  (interactive)
  (dolist (item auximap-all-entries)
    (plist-put item :mark nil))
  (auximap--render-buffer)
  (message "全マークを解除しました。"))

(defun auximap-toggle-all-marks ()
  "Toggle marks on all emails."
  (interactive)
  (dolist (item auximap-all-entries)
    (let ((m (plist-get item :mark)))
      (plist-put item :mark (if m nil ?*))))
  (auximap--render-buffer)
  (message "マークを反転しました。"))

;;; ----------------------------------------------------------------------------
;;; Keyword Batch Marking & Filtering
;;; ----------------------------------------------------------------------------

(defun auximap-mark-by-keyword (keyword)
  "Mark emails matching KEYWORD with `*`."
  (interactive "sMark emails matching keyword (subject/from): ")
  (if (string-empty-p keyword)
      (message "キーワードが空です。")
    (let ((count 0)
          (kw (downcase keyword)))
      (dolist (item auximap-all-entries)
        (let ((subj (downcase (plist-get item :subject)))
              (from (downcase (plist-get item :from))))
          (when (or (string-match-p (regexp-quote kw) subj)
                    (string-match-p (regexp-quote kw) from))
            (plist-put item :mark ?*)
            (cl-incf count))))
      (auximap--render-buffer)
      (message "キーワード「%s」に一致する %d 件のメールをマークしました。" keyword count))))

(defun auximap-mark-delete-by-keyword (keyword)
  "Mark emails matching KEYWORD with `D` for deletion."
  (interactive "sMark for deletion matching keyword (subject/from): ")
  (if (string-empty-p keyword)
      (message "キーワードが空です。")
    (let ((count 0)
          (kw (downcase keyword)))
      (dolist (item auximap-all-entries)
        (let ((subj (downcase (plist-get item :subject)))
              (from (downcase (plist-get item :from))))
          (when (or (string-match-p (regexp-quote kw) subj)
                    (string-match-p (regexp-quote kw) from))
            (plist-put item :mark ?D)
            (cl-incf count))))
      (auximap--render-buffer)
      (message "キーワード「%s」に一致する %d 件を削除マーク(D)しました。" keyword count))))

(defun auximap-unmark-by-keyword (keyword)
  "Unmark emails matching KEYWORD."
  (interactive "sUnmark emails matching keyword (subject/from): ")
  (if (string-empty-p keyword)
      (message "キーワードが空です。")
    (let ((count 0)
          (kw (downcase keyword)))
      (dolist (item auximap-all-entries)
        (when (plist-get item :mark)
          (let ((subj (downcase (plist-get item :subject)))
                (from (downcase (plist-get item :from))))
            (when (or (string-match-p (regexp-quote kw) subj)
                      (string-match-p (regexp-quote kw) from))
              (plist-put item :mark nil)
              (cl-incf count)))))
      (auximap--render-buffer)
      (message "キーワード「%s」に一致する %d 件のマークを解除しました。" keyword count))))

(defun auximap-filter-by-keyword (keyword)
  "Filter displayed emails by KEYWORD. Press empty RET or `//` to clear."
  (interactive "sFilter keyword (empty to clear): ")
  (if (or (string-empty-p keyword) (string-equal keyword "//"))
      (progn
        (setq auximap-current-filter nil)
        (auximap--render-buffer)
        (message "フィルタを解除しました（全件表示）。"))
    (setq auximap-current-filter keyword)
    (auximap--render-buffer)
    (message "フィルタ適用: 「%s」（解除するには [/] で空Enter）" keyword)))

;;; ----------------------------------------------------------------------------
;;; Execution: Safe Delete (Move to Trash)
;;; ----------------------------------------------------------------------------

(defun auximap-execute-delete ()
  "Move marked emails (* or D) to Trash with y/n confirmation."
  (interactive)
  (let* ((marked-entries (cl-remove-if-not (lambda (e) (plist-get e :mark)) auximap-all-entries))
         ;; If no marks exist, prompt for the email at point
         (targets (if marked-entries
                      marked-entries
                    (when-let ((curr (auximap-current-entry)))
                      (list curr)))))
    (if (null targets)
        (message "削除対象のメールがありません（SPC または d でマークしてから x を押してください）。")
      (let* ((count (length targets))
             (is-trash (string-match-p "trash\\|ゴミ箱\\|ごみ箱" (downcase auximap-current-folder)))
             ;; Build summary prompt
             (sample-titles (mapconcat
                             (lambda (e)
                               (format "  - [%s] %s" (plist-get e :uid) (truncate-string-to-width (plist-get e :subject) 50 0 nil "...")))
                             (cl-subseq targets 0 (min 5 count))
                             "\n"))
             (more-str (if (> count 5) (format "\n  ... 他 %d 件" (- count 5)) ""))
             (warning (if is-trash
                          "【警告】ゴミ箱フォルダ内のメールは完全消去されます！\n"
                        ""))
             (prompt (format "%sゴミ箱へ移動するメール (%d件):\n%s%s\n本当に移動しますか？ (y or n) "
                             warning count sample-titles more-str)))

        (if (not (y-or-n-p prompt))
            (message "削除を中止しました。")
          ;; Execute bulk deletion via auximap.exe delete <account/folder> <uid1> <uid2> ...
          (message "サーバーのゴミ箱へ移動中... (%d件)" count)
          (let* ((target-folder (format "%s/%s" auximap-current-account auximap-current-folder))
                 (uids (mapcar (lambda (e) (plist-get e :uid)) targets))
                 (exit-code (apply #'auximap--call-process auximap-current-account "delete" target-folder uids)))
            (if (not (zerop exit-code))
                (message "ゴミ箱への移動に失敗しました (exit code %d)。詳細は auximap.log をご確認ください。" exit-code)
              ;; Remove targets from auximap-all-entries
              (setq auximap-all-entries
                    (cl-remove-if (lambda (e) (member (plist-get e :uid) uids)) auximap-all-entries))
              (auximap--render-buffer)
              (message "%d 件のメールをゴミ箱へ移動しました。" count)
              (when auximap-auto-preview
                (auximap-preview-current)))))))))

;;; ----------------------------------------------------------------------------
;;; Preview & Body Display
;;; ----------------------------------------------------------------------------

(defun auximap-preview-current ()
  "Preview the email at point in the view buffer."
  (interactive)
  (when-let ((entry (auximap-current-entry)))
    (let ((uid (plist-get entry :uid)))
      (unless (equal uid auximap--last-previewed-uid)
        (setq auximap--last-previewed-uid uid)
        (auximap-display-body auximap-current-account auximap-current-folder uid)))))

(defun auximap-display-body (account folder uid)
  "Display the body of UID for ACCOUNT and FOLDER in the view buffer."
  (let* ((cache-key (format "%s/%s/%s" account folder uid))
         (cache-file (expand-file-name (format "%s_%s_%s.txt" account (replace-regexp-in-string "[/\\\\]" "_" folder) uid) auximap-cache-dir))
         (view-buf (get-buffer "*auximap-view*")))
    (when (get-buffer-window view-buf)
      (let ((body (or (gethash cache-key auximap--memory-cache)
                      (if (file-exists-p cache-file)
                          (with-temp-buffer
                            (let ((coding-system-for-read 'utf-8-dos))
                              (insert-file-contents cache-file))
                            (buffer-string))
                        (let ((fetched (auximap--fetch-body account folder uid cache-file)))
                          (when fetched
                            (puthash cache-key fetched auximap--memory-cache))
                          fetched)))))
        (when body
          (with-current-buffer view-buf
            (let ((inhibit-read-only t))
              (erase-buffer)
              (insert body)
              (goto-char (point-min)))))))))

(defun auximap--fetch-body (account folder uid cache-file)
  "Call auximap.exe read to fetch email body into CACHE-FILE."
  (let* ((target (format "%s/%s" account folder))
         (exit-code (auximap--call-process account "read" target uid cache-file)))
    (if (and (zerop exit-code) (file-exists-p cache-file))
        (with-temp-buffer
          (let ((coding-system-for-read 'utf-8-dos))
            (insert-file-contents cache-file))
          (buffer-string))
      (message "Failed to fetch email body for UID %s" uid)
      nil)))

;;; ----------------------------------------------------------------------------
;;; Navigation & Helpers
;;; ----------------------------------------------------------------------------

(defun auximap-next-email ()
  "Move to the next email and auto-preview."
  (interactive)
  (forward-line 1)
  (if (eobp)
      (forward-line -1)
    (when auximap-auto-preview
      (auximap-preview-current))))

(defun auximap-prev-email ()
  "Move to the previous email and auto-preview."
  (interactive)
  (forward-line -1)
  (while (and (not (bobp)) (null (auximap-current-entry)))
    (forward-line -1))
  (when auximap-auto-preview
    (auximap-preview-current)))

(defun auximap-scroll-view-up ()
  "Scroll view buffer down (next page)."
  (interactive)
  (when-let ((win (get-buffer-window "*auximap-view*")))
    (with-selected-window win
      (scroll-up-command))))

(defun auximap-scroll-view-down ()
  "Scroll view buffer up (previous page)."
  (interactive)
  (when-let ((win (get-buffer-window "*auximap-view*")))
    (with-selected-window win
      (scroll-down-command))))

(defun auximap-focus-view ()
  "Focus on the view buffer."
  (interactive)
  (when-let ((win (get-buffer-window "*auximap-view*")))
    (select-window win)))

(defun auximap-view-quit-to-list ()
  "Quit view buffer and focus back on list buffer."
  (interactive)
  (when-let ((list-win (get-window-with-predicate
                        (lambda (w)
                          (string-prefix-p "*auximap:" (buffer-name (window-buffer w)))))))
    (select-window list-win)))

(defun auximap-view-next ()
  "View next email from view buffer."
  (interactive)
  (auximap-view-quit-to-list)
  (auximap-next-email)
  (auximap-focus-view))

(defun auximap-view-prev ()
  "View previous email from view buffer."
  (interactive)
  (auximap-view-quit-to-list)
  (auximap-prev-email)
  (auximap-focus-view))

(defvar auximap--folder-cache (make-hash-table :test 'equal)
  "Cache of folder lists per account.")

(defun auximap--fetch-folders (account &optional force-refresh)
  "Fetch IMAP folder list for ACCOUNT dynamically from server (with cache).
If FORCE-REFRESH is non-nil, bypass cache and re-query server."
  (let ((cached (gethash account auximap--folder-cache)))
    (if (and cached (not force-refresh))
        cached
      (let* ((target-dir (or auximap-cache-dir temporary-file-directory))
             (temp-file (expand-file-name (format "folders_%s.txt" account) target-dir))
             (folders nil))
        (unless (file-exists-p target-dir)
          (make-directory target-dir t))
        (message "サーバーからフォルダ一覧を取得中 (%s)..." account)
        (let ((exit-code (auximap--call-process account "list" account temp-file)))
          (if (and (zerop exit-code) (file-exists-p temp-file))
              (progn
                (with-temp-buffer
                  (let ((coding-system-for-read 'utf-8-dos))
                    (insert-file-contents temp-file))
                  (goto-char (point-min))
                  (while (not (eobp))
                    (let ((line (string-trim (buffer-substring-no-properties (line-beginning-position) (line-end-position)))))
                      (when (and (string-prefix-p "\"" line)
                                 (string-match "\"\\([^\"]+\\)\"" line))
                        (push (match-string 1 line) folders)))
                    (forward-line 1)))
                (setq folders (nreverse folders))
                (when folders
                  (puthash account folders auximap--folder-cache))
                folders)
            ;; 取得失敗時のフォールバック
            (if (string-equal (downcase account) "gmail")
                '("INBOX" "[Gmail]/送信済みメール" "[Gmail]/下書き" "[Gmail]/ゴミ箱" "[Gmail]/迷惑メール" "[Gmail]/すべてのメール")
              '("INBOX" "Trash" "Sent" "Drafts" "Junk" "Archive"))))))))

(defun auximap-change-folder (&optional refresh-folders)
  "Prompt to change IMAP folder with completion.
With prefix arg REFRESH-FOLDERS (C-u f), refresh folder list from server."
  (interactive "P")
  (let* ((candidates (auximap--fetch-folders auximap-current-account refresh-folders))
         (prompt (format "フォルダ切替 (現在: %s): " auximap-current-folder))
         (chosen (completing-read prompt candidates nil nil nil nil auximap-current-folder)))
    (when (and chosen (not (string-empty-p chosen)))
      (setq auximap-current-folder chosen)
      (setq auximap-current-filter nil)
      (auximap-refresh))))

(defun auximap-change-account ()
  "Prompt to change IMAP account with completion dynamically parsed from auximap.ini."
  (interactive)
  (let* ((candidates (auximap--fetch-accounts))
         (prompt (format "アカウント切替 (現在: %s): " (or auximap-current-account "未設定")))
         (default-val (or (and candidates (car (remove auximap-current-account candidates)))
                          (and candidates (car candidates))
                          auximap-current-account))
         (chosen (if candidates
                     (completing-read prompt candidates nil nil nil nil default-val)
                   (read-string prompt nil nil auximap-current-account))))
    (when (and chosen (not (string-empty-p chosen)))
      (setq auximap-current-account chosen)
      (setq auximap-current-folder "INBOX")
      (setq auximap-current-filter nil)
      (auximap-refresh))))

(defun auximap-quit ()
  "Quit auximap and restore window configuration."
  (interactive)
  (let ((config auximap-saved-window-config))
    (kill-buffer (get-buffer-create "*auximap-view*"))
    (kill-buffer (current-buffer))
    (when config
      (set-window-configuration config))))

(defun auximap-toggle-auto-preview ()
  "Toggle automatic email body preview when moving cursor."
  (interactive)
  (setq auximap-auto-preview (not auximap-auto-preview))
  (message "自動プレビュー: %s" (if auximap-auto-preview "有効 (ON: カーソル移動で自動表示)" "無効 (OFF: 'v' または Enter で表示)")))

(defun auximap-toggle ()
  "Toggle auximap email reader (open if closed, close if open)."
  (interactive)
  (let ((list-buf (cl-find-if (lambda (b) (string-prefix-p "*auximap:" (buffer-name b))) (buffer-list))))
    (if (and list-buf (get-buffer-window list-buf))
        (with-selected-window (get-buffer-window list-buf)
          (auximap-quit))
      (auximap))))

(provide 'auximap)
;;; auximap.el ends here
