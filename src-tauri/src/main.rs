// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn arg_value(args: &[String], key: &str) -> Option<String> {
    let i = args.iter().position(|a| a == key)?;
    args.get(i + 1).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // 无 GUI 模式：依赖安装 / 自检（CI、命令行验证用）
    if args.iter().any(|a| a == "--install-tools" || a == "--install") {
        let only = arg_value(&args, "--install");
        let model = arg_value(&args, "--model");
        let has_model_flag = args.iter().any(|a| a == "--model");
        let code = umi_downloader_lib::cli_install_tools(
            only.as_deref(),
            if has_model_flag {
                model.as_deref().or(Some("base"))
            } else {
                None
            },
        );
        std::process::exit(code);
    }

    if args.iter().any(|a| a == "--selftest") {
        #[cfg(feature = "selftest")]
        {
            let deep = !args.iter().any(|a| a == "--quick");
            let code = umi_downloader_lib::cli_selftest(deep);
            std::process::exit(code);
        }
        #[cfg(not(feature = "selftest"))]
        {
            eprintln!("正式版本未包含自检模块，请使用 --features selftest 构建开发版");
            std::process::exit(2);
        }
    }

    umi_downloader_lib::run()
}
