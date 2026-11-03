// Ask the shell for registered handlers, including packaged applications.
export const associations = `
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;
public class FileApps {
  [ComImport, Guid("973810AE-9599-4B88-9E4D-6EE98C9552DA"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface Handlers { [PreserveSig] int Next(uint count, out Handler handler, out uint fetched); }
  [ComImport, Guid("F04061AC-1659-4A3F-A954-775AA57FC083"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface Handler {
    void GetName([MarshalAs(UnmanagedType.LPWStr)] out string name);
    void GetUIName([MarshalAs(UnmanagedType.LPWStr)] out string name);
    void GetIconLocation([MarshalAs(UnmanagedType.LPWStr)] out string path, out int index);
    [PreserveSig] int IsRecommended();
    void MakeDefault([MarshalAs(UnmanagedType.LPWStr)] string description);
    void Invoke(IDataObject data);
  }
  [ComImport, Guid("43826D1E-E718-42EE-BC55-A1E261C37BFE"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface ShellItem { void BindToHandler(IntPtr context, ref Guid handler, ref Guid iid, out IntPtr result); }
  [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
  static extern void SHAssocEnumHandlers(string extension, uint filter, out Handlers handlers);
  [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
  static extern void SHCreateItemFromParsingName(string path, IntPtr context, ref Guid iid, out ShellItem item);
  [DllImport("shlwapi.dll", CharSet = CharSet.Unicode)]
  static extern uint AssocQueryString(uint flags, uint kind, string association, string extra, StringBuilder value, ref uint length);
  static string Query(string extension, uint kind) {
    uint length = 0; AssocQueryString(0, kind, extension, null, null, ref length);
    if (length == 0) return null;
    var value = new StringBuilder((int)length);
    return AssocQueryString(0, kind, extension, null, value, ref length) == 0 ? value.ToString() : null;
  }
  static List<Handler> Enumerate(string path) {
    var result = new List<Handler>();
    string extension = System.IO.Path.GetExtension(path);
    if (extension.Length == 0) return result;
    Handlers handlers; SHAssocEnumHandlers(extension, 0, out handlers);
    if (handlers == null) return result;
    Handler handler; uint fetched;
    while (handlers.Next(1, out handler, out fetched) == 0 && fetched == 1) result.Add(handler);
    return result;
  }
  public static object List(string path) {
    var apps = new List<object>();
    foreach (var handler in Enumerate(path)) {
      string id, name; handler.GetName(out id); handler.GetUIName(out name);
      apps.Add(new { id, name, kind = "handler" });
    }
    string extension = System.IO.Path.GetExtension(path);
    return new { defaultId = Query(extension, 2), defaultName = Query(extension, 4), apps };
  }
  public static void Open(string path, string id) {
    foreach (var handler in Enumerate(path)) {
      string name; handler.GetName(out name);
      if (name != id) continue;
      Guid itemId = new Guid("43826D1E-E718-42EE-BC55-A1E261C37BFE");
      ShellItem item; SHCreateItemFromParsingName(path, IntPtr.Zero, ref itemId, out item);
      Guid dataHandler = new Guid("B8C0BD9F-ED24-455C-83E6-D5390C4FE8C4");
      Guid dataId = new Guid("0000010E-0000-0000-C000-000000000046");
      IntPtr pointer; item.BindToHandler(IntPtr.Zero, ref dataHandler, ref dataId, out pointer);
      try { handler.Invoke((IDataObject)Marshal.GetObjectForIUnknown(pointer)); }
      finally { Marshal.Release(pointer); }
      return;
    }
    throw new InvalidOperationException("This application is no longer available. Choose another application.");
  }
}
'@
`;

export const picker = `
Add-Type -AssemblyName System.Windows.Forms
$dialog = New-Object System.Windows.Forms.OpenFileDialog
$dialog.Title = 'Choose an application'
$dialog.Filter = 'Applications (*.exe)|*.exe'
$dialog.CheckFileExists = $true
try {
  if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { $dialog.FileName }
} finally { $dialog.Dispose() }
`;
