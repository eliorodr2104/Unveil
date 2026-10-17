//
//  PhotosBrowser.swift
//  Unveil
//

import Photos
import SwiftUI

/// PhotosBrowser is the Photos panel's content, laid out like the system Files browser beside it:
/// a toolbar (the place on the leading side, a Recents / Albums switch in the middle, the RAW
/// filter and, with limited access, Manage on the trailing side) over an airy grid of photos with
/// their file name and date, or of albums. It draws no background: the panel's glass shows
/// through, as it does behind the Files browser.
///
/// A tap on a photo hands its asset identifier to `onPick`. `isScrollEnabled` false (the panel
/// collapsed) lets the panel's own drag take vertical swipes, while taps still open photos.
struct PhotosBrowser: View {

    @Bindable
    var library: PhotoLibraryStore

    let isScrollEnabled: Bool
    let onPick         : (String) -> Void

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            content
        }
    }

    // MARK: Toolbar

    private var toolbar: some View {
        ZStack {
            CapsulePicker(
                label     : "Photos Scope",
                options   : [("Recents", PhotoLibraryStore.Scope.recents), ("Albums", .albums)],
                selection : $library.scope
            )

            HStack(spacing: 10) {
                leadingItems
                Spacer(minLength: 0)
                trailingItems
            }
        }
        .padding(.horizontal, 28)
        .padding(.top, 12)
        .padding(.bottom, 8)
        .opacity(library.access == .denied ? 0 : 1)
    }

    @ViewBuilder
    private var leadingItems: some View {
        if library.scope == .albums, let album = library.openAlbum {
            Button {
                withAnimation(Self.navigation) { library.closeAlbum() }
            } label: {
                Image(systemName: "chevron.left")
                    .font(.body.weight(.semibold))
                    .frame(width: 28, height: 28)
            }
            .buttonStyle(.glass)
            .accessibilityLabel("Albums")
            .transition(.opacity.combined(with: .scale(scale: 0.8)))

            Text(album.title)
                .font(.headline)
                .lineLimit(1)
                .transition(.opacity)
        } else {
            Text(library.scope == .recents ? "Recents" : "Albums")
                .font(.headline)
                .contentTransition(.opacity)
        }
    }

    @ViewBuilder
    private var trailingItems: some View {
        if library.access == .limited {
            Button("Manage") { library.manageLimitedSelection() }
                .buttonStyle(.glass)
        }

        if showsPhotos {
            Button {
                withAnimation(.smooth(duration: 0.25)) { library.showsOnlyRAW.toggle() }
            } label: {
                Text("RAW")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(library.showsOnlyRAW ? AnyShapeStyle(.tint) : AnyShapeStyle(.primary))
            }
            .buttonStyle(.glass)
            .accessibilityLabel("Show Only RAW Photos")
            .accessibilityAddTraits(library.showsOnlyRAW ? .isSelected : [])
            .transition(.opacity.combined(with: .scale(scale: 0.8)))
        }
    }

    /// navigation is the spring of opening and closing an album, a navigation push and pop.
    private static let navigation = Animation.smooth(duration: 0.35)

    /// scopeShift is how far Recents and Albums slide while they cross-fade: a hint of the
    /// segment's direction, not a page turn.
    private static let scopeShift: CGFloat = 28

    /// showsPhotos is true when the panel shows a photo grid (Recents or an open album), where the
    /// RAW filter applies.
    private var showsPhotos: Bool {
        library.scope == .recents || library.openAlbum != nil
    }

    // MARK: Content

    @ViewBuilder
    private var content: some View {
        switch library.access {
            case .undetermined:
                ContentUnavailableView {
                    Label("Photos", systemImage: "photo.on.rectangle")
                } description: {
                    Text("Allow access to open photos from your library.")
                } actions: {
                    Button("Allow Access") { Task { await library.requestAccess() } }
                        .buttonStyle(.glassProminent)
                }

            case .denied:
                ContentUnavailableView {
                    Label("No Access to Photos", systemImage: "photo.on.rectangle")
                } description: {
                    Text("Allow Unveil to read your photo library in Settings.")
                } actions: {
                    Button("Open Settings") {
                        if let url = URL(string: UIApplication.openSettingsURLString) {
                            UIApplication.shared.open(url)
                        }
                    }
                    .buttonStyle(.glassProminent)
                }

            case .limited, .full:
                // Both scopes stay mounted and only the visible one takes part: switching is
                // instant, nothing is rebuilt, and each keeps its scroll position.
                ZStack {
                    scopeLayer(.recents, enteringFrom: -1) {
                        photoGrid(library.recents)
                    }

                    scopeLayer(.albums, enteringFrom: 1) {
                        albumsLayer
                    }
                }
        }
    }

    /// scopeLayer shows a scope's content while it is the chosen one. Switching cross-fades the
    /// two and slides them a little toward where their segment sits (Recents from the leading
    /// side, Albums from the trailing one), in the picker's own animation.
    private func scopeLayer(
        _ scope          : PhotoLibraryStore.Scope,
        enteringFrom side: CGFloat,
        @ViewBuilder content: () -> some View
    ) -> some View {
        let isShown = library.scope == scope

        return content()
            .opacity(isShown ? 1 : 0)
            .offset(x: isShown ? 0 : side * Self.scopeShift)
            .allowsHitTesting(isShown)
            .accessibilityHidden(!isShown)
    }

    /// albumsLayer is the album list, with an open album pushed over it like a navigation
    /// stack: the album's page comes in from the trailing edge while the list drifts back and
    /// fades, and going back reverses it. Both stay mounted and are only moved, so the push runs
    /// the same however fast the album's photos arrive, and the list is found where it was.
    private var albumsLayer: some View {
        let isAlbumOpen = library.openAlbum != nil

        return ZStack {
            albumGrid
                .offset(x: isAlbumOpen ? -Self.scopeShift * 2 : 0)
                .opacity(isAlbumOpen ? 0 : 1)
                .allowsHitTesting(!isAlbumOpen)
                .accessibilityHidden(isAlbumOpen)

            photoGrid(library.albumPhotos)
                .visualEffect { content, proxy in
                    content.offset(x: isAlbumOpen ? 0 : proxy.size.width)
                }
                .opacity(isAlbumOpen ? 1 : 0)
                .allowsHitTesting(isAlbumOpen)
                .accessibilityHidden(!isAlbumOpen)
        }
    }

    /// columns gives every item a cell like the Files browser's: room for an 88-point thumbnail
    /// and two lines of name, with generous gaps.
    private static let columns = [GridItem(.adaptive(minimum: 124, maximum: 150), spacing: 12)]

    /// photoGrid shows a photo list, a spinner while its first fetch runs, or an empty state. A
    /// list for a new query (the RAW filter turned on or off) cross-fades in as a fresh grid when
    /// its fetch lands; a refetch of the same query keeps the grid and its scroll position.
    private func photoGrid(_ photos: PhotoList?) -> some View {
        ZStack {
            photoGridContent(photos)
                .id(photos?.query)
                .transition(.opacity)
        }
        .animation(.smooth(duration: 0.3), value: photos?.query)
    }

    @ViewBuilder
    private func photoGridContent(_ photos: PhotoList?) -> some View {
        if let photos, photos.count == 0 {
            ContentUnavailableView(
                library.showsOnlyRAW ? "No RAW Photos" : "No Photos",
                systemImage : "photo",
                description : Text(library.showsOnlyRAW ? "RAW and ProRAW photos appear here." : "Photos you take or save appear here.")
            )
        } else if let photos {
            ScrollView {
                LazyVGrid(columns: Self.columns, spacing: 28) {
                    ForEach(0 ..< photos.count, id: \.self) { index in
                        let asset = photos.asset(at: index)

                        Button {
                            onPick(asset.localIdentifier)
                        } label: {
                            PhotoCell(photos: photos, index: index)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 16)
                .padding(.bottom, 40)
            }
            .scrollDisabled(!isScrollEnabled)
        } else {
            ProgressView()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    /// albumGrid shows the albums as they arrive, a spinner before the first batch, or an empty
    /// state. Cards are keyed by album, so a batch landing adds cards without moving the others.
    @ViewBuilder
    private var albumGrid: some View {
        if let albums = library.albums, albums.isEmpty {
            ContentUnavailableView("No Albums", systemImage: "rectangle.stack", description: Text("Albums with photos appear here."))
        } else if let albums = library.albums {
            ScrollView {
                LazyVGrid(columns: Self.columns, spacing: 28) {
                    ForEach(albums) { album in
                        Button {
                            withAnimation(Self.navigation) { library.open(album) }
                        } label: {
                            AlbumCell(album: album)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 16)
                .padding(.bottom, 40)
            }
            .scrollDisabled(!isScrollEnabled)
        } else {
            ProgressView()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

/// PhotoCell is one photo the way Files shows a file: the thumbnail at its own aspect ratio in an
/// 88-point box with a badge naming the format of the file it would open (JPEG, HEIC, DNG, RAF...),
/// then the file name and the date. The name and the badge come from the asset's resources, read off
/// the main actor.
private struct PhotoCell: View {

    let photos: PhotoList
    let index : Int

    @State
    private var details: PhotoDetails?

    var body: some View {
        let asset = photos.asset(at: index)

        VStack(spacing: 6) {
            PhotoThumbnail(asset: asset, contentMode: .fit, format: details?.format)
                .frame(width: Self.thumbnailSide, height: Self.thumbnailSide)
                .padding(.bottom, 6)

            CellCaption(
                title             : details?.wrappableName ?? " ",
                subtitle          : asset.creationDate?.formatted(Self.dateStyle) ?? " ",
                accessibilityTitle: details?.name ?? "",
                truncation        : .middle
            )
        }
        .frame(maxWidth: .infinity)
        .contentShape(.rect)
        .task(id: asset.localIdentifier) {
            details = await PhotoDetails.load(from: photos, at: index)
        }
        .accessibilityElement(children: .combine)
    }

    static let thumbnailSide: CGFloat = 88

    /// dateStyle is the Files browser's: two-digit day, month and year, then the time.
    private static let dateStyle = Date.FormatStyle()
        .day(.twoDigits)
        .month(.twoDigits)
        .year(.twoDigits)
        .hour()
        .minute()
}

/// PhotoDetails is what a cell reads from the asset's resources: the name of the file it would
/// open (without the extension, as Files shows it) and its format.
nonisolated private struct PhotoDetails: Equatable, Sendable {

    let name  : String
    let format: String?

    /// wrappableName is the name with a zero-width space after every character, so it wraps at
    /// any character like Files does ("iPhone12Pro_IM / G_1361") instead of being hyphenated
    /// like prose ("campho- / to_3346"), which SwiftUI's Text offers no switch for.
    var wrappableName: String {
        name.map(String.init).joined(separator: "\u{200B}")
    }

    /// load reads the resources of the photo at `index` off the main actor: a resource lookup
    /// per cell is a library query, and a screenful of them stalls scrolling on the main thread.
    @concurrent
    static func load(from photos: PhotoList, at index: Int) async -> PhotoDetails {
        let resources = PHAssetResource.assetResources(for: photos.asset(at: index))
        let kinds     = resources.map { PhotoResourceChoice.Kind(type: $0.type, typeIdentifier: $0.uniformTypeIdentifier) }
        let chosen    = PhotoResourceChoice.preferredIndex(in: kinds)

        guard let chosen else {
            return PhotoDetails(name: "Photo", format: nil)
        }

        let fileName = resources[chosen].originalFilename
        return PhotoDetails(
            name   : (fileName as NSString).deletingPathExtension,
            format : PhotoResourceChoice.formatName(fileName: fileName, typeIdentifier: kinds[chosen].typeIdentifier)
        )
    }
}

/// AlbumCell is one album as a small stack of cards: its newest photo filling a rounded square
/// over two offset sheets, then the title and the photo count. It is the photo cell's size.
private struct AlbumCell: View {

    let album: PhotoAlbum

    private let side = PhotoCell.thumbnailSide

    var body: some View {
        VStack(spacing: 6) {
            ZStack {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(.white.opacity(0.10))
                    .frame(width: side - 16, height: side - 16)
                    .offset(y: -8)

                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(.white.opacity(0.18))
                    .frame(width: side - 8, height: side - 8)
                    .offset(y: -4)

                Group {
                    if let keyPhoto = album.keyPhoto {
                        PhotoThumbnail(asset: keyPhoto, contentMode: .fill, format: nil)
                    } else {
                        Color.secondary.opacity(0.3)
                    }
                }
                .frame(width: side, height: side)
                .clipShape(.rect(cornerRadius: 8, style: .continuous))
            }
            .frame(width: side, height: side, alignment: .bottom)
            .padding(.bottom, 6)

            CellCaption(
                title             : album.title,
                subtitle          : album.count == 1 ? "1 photo" : "\(album.count) photos",
                accessibilityTitle: album.title,
                truncation        : .tail
            )
        }
        .frame(maxWidth: .infinity)
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
    }
}

/// CellCaption is a cell's name and the line under it (date, photo count). The line follows the
/// name directly, one line or two, but the caption always takes the height of a two-line name:
/// the spare room goes below it, so a name arriving after the cell never changes the row's height
/// and shifts the grid under a scrolling finger.
private struct CellCaption: View {

    let title             : String
    let subtitle          : String
    let accessibilityTitle: String
    let truncation        : Text.TruncationMode

    var body: some View {
        ZStack(alignment: .top) {
            lines(title: "\n", subtitle: " ")
                .hidden()

            lines(title: title, subtitle: subtitle)
        }
    }

    private func lines(title: String, subtitle: String) -> some View {
        VStack(spacing: 6) {
            Text(title)
                .font(.callout)
                .lineLimit(2)
                .truncationMode(truncation)
                .multilineTextAlignment(.center)
                .accessibilityLabel(accessibilityTitle)

            Text(subtitle)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}

/// PhotoThumbnail loads an asset's thumbnail at the pixel size its 88-point box needs, first a
/// quick low-quality one, then the sharp one. Leaving the screen cancels the request, so a fast
/// scroll through thousands of photos only loads what stops in view. The first image fades in
/// rather than popping. The format badge sits on the photo's own corner, which is why it is drawn
/// here, on the fitted image.
private struct PhotoThumbnail: View {

    let asset      : PHAsset
    let contentMode: ContentMode
    let format     : String?

    @Environment(\.displayScale)
    private var displayScale

    @State
    private var image: UIImage?

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .aspectRatio(contentMode: contentMode)
                    .clipShape(.rect(cornerRadius: contentMode == .fit ? 4 : 0))
                    // A hairline instead of a shadow: a shadow on an image is an offscreen pass
                    // per cell, a screenful of which made every frame of the moving panel costly.
                    .overlay {
                        if contentMode == .fit {
                            RoundedRectangle(cornerRadius: 4)
                                .strokeBorder(.white.opacity(0.14), lineWidth: 0.5)
                        }
                    }
                    .overlay(alignment: .bottomTrailing) {
                        if let format {
                            // One line always: on a narrow photo the badge overhangs it instead.
                            Text(format)
                                .font(.caption2.weight(.bold))
                                .lineLimit(1)
                                .fixedSize()
                                .padding(.horizontal, 5)
                                .padding(.vertical, 2)
                                .background(.black.opacity(0.55), in: .capsule)
                                .padding(5)
                                .transition(.opacity)
                        }
                    }
                    .transition(.opacity)
            } else {
                Color.clear
            }
        }
        .animation(.easeOut(duration: 0.2), value: image == nil)
        .animation(.easeOut(duration: 0.2), value: format)
        .task(id: asset.localIdentifier) {
            let side = PhotoCell.thumbnailSide * displayScale
            for await thumbnail in PhotoThumbnails.images(for: asset, pixelSide: side) {
                image = thumbnail
            }
        }
    }
}

/// PhotoThumbnails asks Photos for thumbnails through one shared caching manager.
nonisolated enum PhotoThumbnails {

    /// manager is shared by every cell. PHImageManager is documented as safe to use from any
    /// thread, which is what lets a cancelled stream cancel its request from its own context.
    private static let manager = PHCachingImageManager()

    /// images yields the opportunistic low-quality image, then the final one, and ends; ending
    /// the iteration early cancels the request.
    static func images(for asset: PHAsset, pixelSide: CGFloat) -> AsyncStream<UIImage> {
        AsyncStream { continuation in
            let options = PHImageRequestOptions()
            options.deliveryMode           = .opportunistic
            options.resizeMode             = .fast
            options.isNetworkAccessAllowed = true

            let request = manager.requestImage(
                for         : asset,
                targetSize  : CGSize(width: pixelSide, height: pixelSide),
                contentMode : .aspectFit,
                options     : options
            ) { image, info in
                if let image {
                    continuation.yield(image)
                }
                if (info?[PHImageResultIsDegradedKey] as? Bool) != true {
                    continuation.finish()
                }
            }

            continuation.onTermination = { _ in manager.cancelImageRequest(request) }
        }
    }
}
