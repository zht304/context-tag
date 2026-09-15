-- ctxtag: C/C++ context tag analyzer for Neovim
local M = {}

M.config = {
    cmd = nil,          -- path to ctxtag-lsp binary, auto-detected if nil
    db_path = nil,      -- auto: project_root/.ctxtag.db
    root_dir = nil,     -- auto-detected
}

M.setup = function(opts)
    opts = opts or {}
    vim.tbl_deep_extend("force", M.config, opts)

    -- Auto-start LSP on C/C++ buffers
    vim.api.nvim_create_autocmd("FileType", {
        pattern = { "c", "cpp", "objc", "objcpp" },
        callback = function(args)
            M.start_lsp(args.buf)
        end,
    })
end

M.start_lsp = function(bufnr)
    bufnr = bufnr or 0
    local root = M.find_root(bufnr)
    if not root then
        return
    end

    local cmd = M.config.cmd or vim.fn.exepath("ctx-lsp")
    if cmd == "" then
        vim.notify("[ctxtag] ctx-lsp binary not found. Build with: cargo build --release", vim.log.levels.WARN)
        return
    end

    local config = {
        name = "ctxtag",
        cmd = { cmd },
        root_dir = root,
        settings = {
            ctxtag = {
                db_path = root .. "/.ctxtag.db",
            },
        },
        on_attach = M.on_attach,
        capabilities = {
            textDocument = {
                definition = {},
                references = {},
                documentSymbol = {},
                hover = {},
                documentHighlight = {},
            },
            workspace = {
                symbol = {},
            },
        },
    }

    -- Use built-in nvim_lsp if available
    if pcall(require, "lspconfig") then
        -- Define ctxtag as a custom server
        local lspconfig = require("lspconfig")
        local util = require("lspconfig.util")

        local servers = util.available_servers()
        local has_ctxtag = false
        for _, s in ipairs(servers) do
            if s == "ctxtag" then
                has_ctxtag = true
                break
            end
        end

        if not has_ctxtag then
            lspconfig.configs.ctxtag = {
                default_config = {
                    cmd = { cmd },
                    filetypes = { "c", "cpp", "objc", "objcpp" },
                    root_dir = function(fname)
                        return M.find_root(0)
                    end,
                    settings = {},
                },
                docs = {
                    description = "C/C++ context tag analyzer",
                },
            }
        end

        lspconfig.ctxtag.setup({
            on_attach = M.on_attach,
        })
    end
end

M.on_attach = function(client, bufnr)
    -- Default keymaps
    local keymap = vim.keymap.set

    keymap("n", "gd", vim.lsp.buf.definition, { buffer = bufnr, desc = "Go to definition" })
    keymap("n", "gr", vim.lsp.buf.references, { buffer = bufnr, desc = "Find references" })
    keymap("n", "K", vim.lsp.buf.hover, { buffer = bufnr, desc = "Hover info" })
    keymap("n", "gI", vim.lsp.buf.implementation, { buffer = bufnr, desc = "Go to implementation" })

    -- Document symbols
    keymap("n", "<leader>fs", function()
        vim.lsp.buf.document_symbol()
    end, { buffer = bufnr, desc = "Document symbols" })

    -- Workspace symbols
    keymap("n", "<leader>fS", function()
        vim.lsp.buf.workspace_symbol()
    end, { buffer = bufnr, desc = "Workspace symbols" })
end

M.find_root = function(bufnr)
    bufnr = bufnr or 0
    local fname = vim.api.nvim_buf_get_name(bufnr)
    if fname == "" then
        return nil
    end

    -- Look for project markers
    for _, marker in ipairs({ ".git", "CMakeLists.txt", "Makefile", "configure.ac", "Cargo.toml", ".ctxtag" }) do
        local root = vim.fs.root(fname, marker)
        if root then
            return root
        end
    end

    -- Fallback: directory of the file
    return vim.fn.fnamemodify(fname, ":p:h")
end

-- Manual index command
M.index = function(root_dir)
    root_dir = root_dir or M.find_root(0)
    if not root_dir then
        vim.notify("[ctxtag] No project root found", vim.log.levels.WARN)
        return
    end

    local cmd = vim.fn.exepath("ctx")
    if cmd == "" then
        vim.notify("[ctxtag] ctx binary not found", vim.log.levels.WARN)
        return
    end

    vim.notify("[ctxtag] Indexing " .. root_dir, vim.log.levels.INFO)

    vim.fn.jobstart({
        cmd, "index", root_dir,
    }, {
        on_stdout = function(_, data, _)
            for _, line in ipairs(data) do
                vim.notify("[ctxtag] " .. line, vim.log.levels.INFO)
            end
        end,
        on_exit = function(_, code)
            if code == 0 then
                vim.notify("[ctxtag] Indexing complete", vim.log.levels.INFO)
            else
                vim.notify("[ctxtag] Indexing failed (code " .. code .. ")", vim.log.levels.ERROR)
            end
        end,
        stdout = true,
    })
end

return M
