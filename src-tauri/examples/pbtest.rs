// 剪贴板写入测试：模拟 copy_file_with_title 的 macOS 逻辑
#[cfg(target_os = "macos")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("/Users/renshi/Projects/wmessage/README.md");
    let title = args.get(2).map(String::as_str).unwrap_or("测试标题");

    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSString, NSURL};

    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();

    // 1) public.file-url
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let url_obj: objc2::rc::Retained<ProtocolObject<dyn NSPasteboardWriting>> =
        ProtocolObject::from_retained(url);
    let objs = NSArray::from_retained_slice(&[url_obj]);
    let ok1 = pb.writeObjects(&objs);

    // 2) NSFilenamesPboardType
    let path_str = NSString::from_str(path);
    let paths = NSArray::from_retained_slice(&[path_str]);
    let ok2 =
        unsafe { pb.setPropertyList_forType(&paths, &NSString::from_str("NSFilenamesPboardType")) };

    // 3) 标题文本
    let text = NSString::from_str(title);
    let ok3 = pb.setString_forType(&text, unsafe { NSPasteboardTypeString });

    println!("writeObjects={ok1} filenames={ok2} setString={ok3}");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("mac only");
}
