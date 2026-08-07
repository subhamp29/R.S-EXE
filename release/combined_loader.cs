using System;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Text;

/// <summary>
/// R.S EXE Combined Installer — self-extracting executable.
/// Embeds rs-exe.exe (standalone) and setup.exe (NSIS installer) by
/// appending raw binary data after this assembly. At runtime it reads
/// its own file, extracts the two embedded payloads to a temp dir,
/// and presents a choice: Install or Portable.
///
/// Build: csc /platform:x64 /out:R.S-EXE-Combined.exe combined_loader.cs
///         then: append marker + lengths + both exes to the compiled output
/// </summary>
class CombinedLoader
{
    // 32-byte ASCII marker — extremely unlikely to appear inside a .NET PE.
    static readonly byte[] Marker = Encoding.ASCII.GetBytes(
        "R.S-EXE-PAYLOAD-V1-MARKER-936b2f7a");

    static void Main()
    {
        string ownPath = Assembly.GetExecutingAssembly().Location;
        byte[] all = File.ReadAllBytes(ownPath);
        int markerIdx = IndexOf(all, Marker);
        if (markerIdx < 0)
        {
            Console.WriteLine("ERROR: This executable appears to be corrupted (marker not found).");
            Console.WriteLine("Please re-download the installer from GitHub Releases.");
            try { Console.ReadKey(); }
            catch { Console.ReadLine(); }
            Environment.Exit(1);
        }

        int dataStart = markerIdx + Marker.Length;
        if (dataStart + 8 > all.Length)
        {
            Console.WriteLine("ERROR: No payload data found after marker.");
            Environment.Exit(1);
        }

        // Layout after marker:
        //   [4 bytes: rs-exe.exe length, little-endian]
        //   [rs-exe.exe raw bytes]
        //   [4 bytes: setup.exe length, little-endian]
        //   [setup.exe raw bytes]
        int rsExeLen = BitConverter.ToInt32(all, dataStart);
        int setupLen = BitConverter.ToInt32(all, dataStart + 4 + rsExeLen);

        if (rsExeLen <= 0 || setupLen <= 0 ||
            dataStart + 8 + rsExeLen + setupLen > all.Length)
        {
            Console.WriteLine("ERROR: Payload header indicates invalid file sizes.");
            Console.WriteLine("rs-exe.exe: " + rsExeLen + " bytes, setup.exe: " + setupLen + " bytes");
            Environment.Exit(1);
        }

        string tempDir = Path.Combine(Path.GetTempPath(), "R.S EXE Installer");
        Directory.CreateDirectory(tempDir);

        string rsExePath = Path.Combine(tempDir, "rs-exe.exe");
        string setupPath = Path.Combine(tempDir, "setup.exe");

        // Write rs-exe.exe
        byte[] rsExeBytes = new byte[rsExeLen];
        Array.Copy(all, dataStart + 8, rsExeBytes, 0, rsExeLen);
        File.WriteAllBytes(rsExePath, rsExeBytes);

        // Write setup.exe
        byte[] setupBytes = new byte[setupLen];
        Array.Copy(all, dataStart + 8 + rsExeLen, setupBytes, 0, setupLen);
        File.WriteAllBytes(setupPath, setupBytes);

        Console.Title = "R.S EXE v0.1.0 - Installer";
        Console.WriteLine();
        Console.WriteLine("====================================================");
        Console.WriteLine("    R.S EXE v0.1.0 - Windows x64");
        Console.WriteLine("    Desktop Android Virtual Device (AVD) Manager");
        Console.WriteLine("====================================================");
        Console.WriteLine();
        Console.WriteLine("Choose an installation option:");
        Console.WriteLine();
        Console.WriteLine("  [1] Install (creates Start Menu shortcut + uninstaller)");
        Console.WriteLine("  [2] Portable  (standalone exe - no installation needed)");
        Console.WriteLine();
        Console.Write("Enter choice (1 or 2), or press Enter for Installer: ");

        string rawInput = Console.ReadLine();
        string choice = rawInput != null ? rawInput.Trim() : "";
        if (string.IsNullOrEmpty(choice)) choice = "1";

        if (choice == "2" || choice.ToLower() == "portable")
        {
            string desktop = Environment.GetFolderPath(Environment.SpecialFolder.Desktop);
            string dest = Path.Combine(desktop, "R.S EXE.exe");
            File.Copy(rsExePath, dest, true);
            Console.WriteLine();
            Console.WriteLine("Standalone executable copied to:");
            Console.WriteLine("  " + dest);
            Console.WriteLine();
            Console.WriteLine("Double-click this file to run the app anytime.");
            Console.WriteLine("No installation required. No admin rights needed.");
            Console.WriteLine();
            Console.Write("Press any key to exit...");
            try { Console.ReadKey(); }
            catch { Console.ReadLine(); }
        }
        else
        {
            Console.WriteLine();
            Console.WriteLine("Starting installer...");
            try
            {
                ProcessStartInfo psi = new ProcessStartInfo
                {
                    FileName = setupPath,
                    UseShellExecute = true,
                    WorkingDirectory = tempDir,
                };
                Process.Start(psi);
            }
            catch (Exception ex)
            {
                Console.WriteLine("Failed to start installer: " + ex.Message);
                Console.WriteLine("Please manually run: " + setupPath);
            try { Console.ReadKey(); }
            catch { Console.ReadLine(); }
            }
        }
    }

    static int IndexOf(byte[] haystack, byte[] needle)
    {
        if (needle.Length == 0 || haystack.Length < needle.Length) return -1;
        for (int i = 0; i <= haystack.Length - needle.Length; i++)
        {
            bool found = true;
            for (int j = 0; j < needle.Length; j++)
            {
                if (haystack[i + j] != needle[j]) { found = false; break; }
            }
            if (found) return i;
        }
        return -1;
    }
}
