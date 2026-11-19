# Cubelink Privacy Policy

**Last updated:** November 19, 2026
**Developer:** Delikesance
**Contact:** open an issue at https://github.com/delikesance/cubelink/issues

Cubelink is a free, open-source Minecraft launcher (https://github.com/delikesance/cubelink). This policy explains what data the app handles. Cubelink has no user accounts of its own, no advertising and no analytics. We do not run servers that collect your data.

## What stays on your device
Cubelink stores the following locally on your computer:
- your game instances, mods, settings and logs;
- your Microsoft/Minecraft sign-in tokens, so you do not have to sign in every time;
- your friends list and the cryptographic keys that identify your device to friends.

We do not receive or have access to this data. Uninstalling Cubelink and deleting its data folder removes it.

## Sign-in with Microsoft
To play Minecraft Java Edition, you sign in with your Microsoft account. The sign-in happens directly between your device and Microsoft and Mojang services (login.microsoftonline.com, api.minecraftservices.com). Cubelink only receives the tokens needed to launch the game and read your Minecraft profile (username and unique ID). Your Microsoft password is never seen by Cubelink. Microsoft's privacy statement applies to this sign-in: https://privacy.microsoft.com

## Network connections the app makes
To work, Cubelink connects to third-party services. These services receive your IP address and the request itself, as with any internet connection:
- **Mojang / Minecraft services:** game versions, game files and libraries.
- **Mod loaders (Fabric, Quilt, Forge, NeoForge):** loader metadata and files.
- **Modrinth and CurseForge:** searching and downloading mods and modpacks.
- **Adoptium:** downloading a Java runtime if one is needed.
- **GitHub (github.com/delikesance/cubelink):** checking for updates, when the app was not installed from the Microsoft Store.
- **Third-party image hosts:** the description of a mod or modpack (from Modrinth or CurseForge) can contain images. They are loaded directly from the host that serves them, which sees your IP address.

Cubelink does not use any relay or discovery service to connect you with friends: no public relay server (n0, the Iroh project, or any other), no DNS or pkarr directory, no DHT. It does not publish your key or your IP address anywhere and does not query a directory to find your friends. Connections between friends are made directly (see "Playing with friends"). To open a port on your router, Cubelink talks to the router itself on your local network (UPnP, NAT-PMP, PCP), never to an internet service.

## Playing with friends
Cubelink lets you share a modpack and a Minecraft world with friends, directly between devices (peer to peer). When you add a friend:
- you exchange a short verification code and the device keys needed to recognise each other;
- your friend can see whether you are online and whether a game is running on your device (the "in game" status), and, when you share, your instance name, mod list and the files needed to join;
- Cubelink can start with your computer session (option in Settings) so that your friends can reach you; while it runs, it answers your friends' presence checks;
- connections are made directly between devices, with no relay in between: your friends' devices see your IP address, and nobody else does. This has a cost, stated plainly: until Cubelink offers discovery through the Mainline DHT and its own relays, two friends who are both behind a strict NAT or CGNAT, without IPv6 and without UPnP, may be unable to reach each other. One of the two must be reachable (IPv6, or a port opened on the router);
- your friend code contains your public key and your IP addresses (up to 8, including your local network and IPv6 addresses): share it only with people you trust. Your friends' devices remember your last known addresses to find you again; nothing is published for this. Older friend codes may contain the address of a relay that no longer exists: Cubelink ignores it;
- with Hearth group worlds, copies of the world are kept on the devices of the other group members.

Only people you accept as friends can ask to join your world. Members of one of your Hearth groups who are not your friends can open a game tunnel to the group's world, and the connection handshake (QUIC) is visible to anyone who knows your public key, without any data being exchanged. You can remove a friend at any time.

## What we do not do
- We do not sell or share your data.
- We do not use advertising, analytics or tracking.
- We do not collect personal information about you on our own servers, because we do not operate any.

## Children
Cubelink is not directed at children under 13 and does not knowingly collect their data. Minecraft accounts for minors are managed through Microsoft family settings.

## Changes
If this policy changes, the new version will be published at the same address with a new date.

## Contact
Questions about this policy: open an issue at https://github.com/delikesance/cubelink/issues.
