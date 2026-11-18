param([string]$Button = "Start Download", [string]$Title = "Download File Info", [int]$WaitMs = 15000)
# Press a button in an IDM window by sending it BM_CLICK (no mouse).
Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public class IdmWin {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumProc f, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public static string Text(IntPtr h) { var s = new StringBuilder(256); GetWindowText(h, s, 256); return s.ToString(); }
  public static IntPtr FindTop(string prefix) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { if (IsWindowVisible(h) && Text(h).StartsWith(prefix)) { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  public static IntPtr FindChild(IntPtr p, string text) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(p, (h, l) => { if (Text(h).Replace("&", "") == text) { found = h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
}
"@
$deadline = (Get-Date).AddMilliseconds($WaitMs)
while ((Get-Date) -lt $deadline) {
    $w = [IdmWin]::FindTop($Title)
    if ($w -ne [IntPtr]::Zero) {
        $b = [IdmWin]::FindChild($w, $Button)
        if ($b -ne [IntPtr]::Zero) {
            [IdmWin]::PostMessage($b, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null  # BM_CLICK
            "pressed $([DateTimeOffset]::Now.ToUnixTimeMilliseconds())"
            exit 0
        }
    }
    Start-Sleep -Milliseconds 30
}
"not found"
exit 1
