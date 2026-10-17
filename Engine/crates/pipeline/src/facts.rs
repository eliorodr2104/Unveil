//! Bounded exact planning facts outlive the original CPU pixels.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;

use lightcraft_develop::{Crop, DevelopSettings, Geometry, RedEye, Spot, Upright};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use crate::geometry::Frame;
use crate::{Plan, RenderRequest, SourceExtent, SourceId, SourceInfo, local, optics, redeye, spots, upright};

const MAX_BYTES: usize = 8 << 20;

/// A new pixel-dependent operation needs one temporary, exact CPU source lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeedsPixels {
    SourceExtent,
    LateralCa,
    Upright,
    SpotSource,
    RedEye,
    Airlight,
}

/// Facts belong to one decoded identity. Only small frame samples and derived values are retained;
/// there is no original image or source Arc here. Geometry-dependent samples keep at most two entries.
pub struct PlanningFacts {
    id: SourceId,
    extent: SourceExtent,
    ca: Option<[f64; 2]>,
    analyses: VecDeque<(String, upright::Analysis)>,
    proxies: VecDeque<(String, Arc<Rgb32f>)>,
    pupils: VecDeque<(String, redeye::Pupil)>,
    offsets: VecDeque<(String, Option<Point>)>,
}

impl PlanningFacts {
    pub fn new(id: SourceId, extent: SourceExtent) -> Self {
        Self { id, extent, ca: None, analyses: VecDeque::new(), proxies: VecDeque::new(), pupils: VecDeque::new(), offsets: VecDeque::new() }
    }

    pub fn id(&self) -> SourceId {
        self.id
    }

    pub fn extent(&self) -> SourceExtent {
        self.extent
    }

    pub fn clear(&mut self) {
        *self = Self::new(self.id, self.extent);
    }

    /// Retained fact/proxy storage, including collection capacity and owned lookup keys.
    pub fn bytes(&self) -> usize {
        let containers = self.analyses.capacity() * std::mem::size_of::<(String, upright::Analysis)>()
            + self.proxies.capacity() * std::mem::size_of::<(String, Arc<Rgb32f>)>()
            + self.pupils.capacity() * std::mem::size_of::<(String, redeye::Pupil)>()
            + self.offsets.capacity() * std::mem::size_of::<(String, Option<Point>)>();
        containers
            + self.ca.map_or(0, |_| std::mem::size_of::<[f64; 2]>())
            + self
                .analyses
                .iter()
                .map(|(key, a)| key.capacity() + (a.vertical.capacity() + a.horizontal.capacity()) * std::mem::size_of::<upright::Segment>())
                .sum::<usize>()
            + self
                .proxies
                .iter()
                .map(|(key, image)| key.capacity() + std::mem::size_of::<Rgb32f>() + 2 * std::mem::size_of::<usize>() + image.data.capacity() * 12)
                .sum::<usize>()
            + self.pupils.iter().map(|(key, _)| key.capacity()).sum::<usize>()
            + self.offsets.iter().map(|(key, _)| key.capacity()).sum::<usize>()
    }

    fn trim(&mut self) {
        while self.proxies.len() > 2 {
            self.proxies.pop_front();
        }
        while self.analyses.len() > 2 {
            self.analyses.pop_front();
        }
        while self.pupils.len() > 256 {
            self.pupils.pop_front();
        }
        while self.offsets.len() > 256 {
            self.offsets.pop_front();
        }
        while self.bytes() > MAX_BYTES {
            if self.proxies.pop_front().is_none()
                && self.analyses.pop_front().is_none()
                && self.pupils.pop_front().is_none()
                && self.offsets.pop_front().is_none()
            {
                self.clear();
                break;
            }
        }
    }
}

pub fn plan_with_facts<'a>(
    info: &SourceInfo,
    settings: &'a DevelopSettings,
    request: &RenderRequest,
    facts: &mut PlanningFacts,
    pixels: Option<&Rgb32f>,
) -> Result<Plan<'a>, NeedsPixels> {
    if pixels.is_some_and(|p| SourceExtent::from(p) != facts.extent || p.data.len() != p.width.saturating_mul(p.height)) {
        return Err(NeedsPixels::SourceExtent);
    }
    crate::plan_using(&mut ResidentFacts { pixels, facts }, info, settings, request)
}

pub(crate) trait FactSource {
    type Error;
    fn extent(&self) -> SourceExtent;
    fn resolve<'a>(&mut self, info: &SourceInfo, s: &'a DevelopSettings) -> Result<Cow<'a, DevelopSettings>, Self::Error>;
    fn frame(&mut self, info: &SourceInfo, s: &DevelopSettings, crop: bool) -> Result<Frame, Self::Error>;
    fn spot(&mut self, info: &SourceInfo, s: &DevelopSettings, spot: &Spot) -> Result<Option<Point>, Self::Error>;
    fn eyes(&mut self, eyes: &[RedEye], s: &DevelopSettings, frame: &Frame, w: usize, h: usize, ppl: f64) -> Result<Vec<redeye::EyeK>, Self::Error>;
    fn airlight(&mut self, info: &SourceInfo, s: &DevelopSettings, frame: &Frame, w: usize, h: usize) -> Result<f32, Self::Error>;
}

pub(crate) struct PixelFacts<'a>(pub &'a Rgb32f);

impl FactSource for PixelFacts<'_> {
    type Error = Infallible;
    fn extent(&self) -> SourceExtent {
        self.0.into()
    }
    fn resolve<'a>(&mut self, info: &SourceInfo, s: &'a DevelopSettings) -> Result<Cow<'a, DevelopSettings>, Self::Error> {
        Ok(upright::resolve(self.0, info, s))
    }
    fn frame(&mut self, info: &SourceInfo, s: &DevelopSettings, crop: bool) -> Result<Frame, Self::Error> {
        Ok(crate::frame_for(self.0, info, s, crop))
    }
    fn spot(&mut self, info: &SourceInfo, s: &DevelopSettings, spot: &Spot) -> Result<Option<Point>, Self::Error> {
        Ok(spots::pick_source(self.0, info, s, spot, None))
    }
    fn eyes(&mut self, eyes: &[RedEye], s: &DevelopSettings, frame: &Frame, w: usize, h: usize, ppl: f64) -> Result<Vec<redeye::EyeK>, Self::Error> {
        Ok(redeye::resolve(self.0, eyes, s.orientation, frame, w, h, ppl))
    }
    fn airlight(&mut self, info: &SourceInfo, s: &DevelopSettings, frame: &Frame, w: usize, h: usize) -> Result<f32, Self::Error> {
        Ok(local::frame_airlight(self.0, info, s, frame, w, h))
    }
}

struct ResidentFacts<'a> {
    pixels: Option<&'a Rgb32f>,
    facts: &'a mut PlanningFacts,
}

impl ResidentFacts<'_> {
    fn ca(&mut self) -> Result<[f64; 2], NeedsPixels> {
        if let Some(ca) = self.facts.ca {
            return Ok(ca);
        }
        let ca = optics::estimate_lateral_ca(self.pixels.ok_or(NeedsPixels::LateralCa)?);
        self.facts.ca = Some(ca);
        Ok(ca)
    }

    fn proxy(&mut self, frame: &Frame, w: usize, h: usize, reason: NeedsPixels) -> Result<Arc<Rgb32f>, NeedsPixels> {
        let key = format!("{frame:?}/{w}/{h}");
        if let Some(index) = self.facts.proxies.iter().position(|(k, _)| k == &key)
            && let Some(entry) = self.facts.proxies.remove(index)
        {
            let image = entry.1.clone();
            self.facts.proxies.push_back(entry);
            return Ok(image);
        }
        let image = Arc::new(frame.sample(self.pixels.ok_or(reason)?, w, h));
        self.facts.proxies.push_back((key, image.clone()));
        self.facts.trim();
        Ok(image)
    }
}

impl FactSource for ResidentFacts<'_> {
    type Error = NeedsPixels;
    fn extent(&self) -> SourceExtent {
        self.facts.extent
    }
    fn resolve<'a>(&mut self, info: &SourceInfo, s: &'a DevelopSettings) -> Result<Cow<'a, DevelopSettings>, Self::Error> {
        let g = &s.geometry;
        if matches!(g.upright, Upright::Off | Upright::Guided) || g.upright_transform.is_some() || !s.section_enabled("geometry") {
            return Ok(Cow::Borrowed(s));
        }
        let mut base = s.clone();
        base.geometry = Geometry::default();
        base.crop = Crop::default();
        let frame = self.frame(info, &base, false)?;
        let key = format!("{frame:?}");
        let analysis = if let Some((_, analysis)) = self.facts.analyses.iter().find(|(k, _)| k == &key) {
            analysis.clone()
        } else {
            let analysis = upright::analyze_frame(self.pixels.ok_or(NeedsPixels::Upright)?, &frame);
            self.facts.analyses.push_back((key, analysis.clone()));
            self.facts.trim();
            analysis
        };
        let mut resolved = s.clone();
        resolved.geometry.upright_transform = Some(upright::solve(g.upright, &analysis));
        Ok(Cow::Owned(resolved))
    }
    fn frame(&mut self, info: &SourceInfo, s: &DevelopSettings, crop: bool) -> Result<Frame, Self::Error> {
        let extent = self.facts.extent;
        let mut frame = Frame::with_lens(extent.width, extent.height, s, crop, info.lens.as_ref());
        if s.optics.remove_ca && s.section_enabled("optics") {
            frame.add_lateral_ca(self.ca()?);
        }
        Ok(frame)
    }
    fn spot(&mut self, info: &SourceInfo, s: &DevelopSettings, spot: &Spot) -> Result<Option<Point>, Self::Error> {
        let frame = self.frame(info, s, true)?;
        let key = format!("{frame:?}/{spot:?}");
        if let Some((_, offset)) = self.facts.offsets.iter().find(|(k, _)| k == &key) {
            return Ok(*offset);
        }
        let (w, h) = frame.fit(512, 512);
        let proxy = self.proxy(&frame, w, h, NeedsPixels::SpotSource)?;
        let offset = spots::pick_source_from_proxy(&proxy, &frame, spot, None);
        self.facts.offsets.push_back((key, offset));
        self.facts.trim();
        Ok(offset)
    }
    fn eyes(&mut self, eyes: &[RedEye], s: &DevelopSettings, frame: &Frame, w: usize, h: usize, ppl: f64) -> Result<Vec<redeye::EyeK>, Self::Error> {
        let mut resolved = Vec::with_capacity(eyes.len());
        for eye in eyes {
            let key = format!("{:?}/{:?}/{:?}/{:?}/{:?}/{:?}/{:?}", s.orientation, frame.ow, frame.oh, eye.center, eye.rx, eye.ry, eye.pet);
            let pupil = if let Some((_, pupil)) = self.facts.pupils.iter().find(|(k, _)| k == &key) {
                *pupil
            } else {
                let pupil = redeye::detect(self.pixels.ok_or(NeedsPixels::RedEye)?, s.orientation, frame.ow, frame.oh, eye);
                self.facts.pupils.push_back((key, pupil));
                self.facts.trim();
                pupil
            };
            resolved.push(redeye::resolved_pupil(eye, pupil, frame, w, h, ppl));
        }
        Ok(resolved)
    }
    fn airlight(&mut self, info: &SourceInfo, s: &DevelopSettings, frame: &Frame, w: usize, h: usize) -> Result<f32, Self::Error> {
        let proxy = self.proxy(frame, w, h, NeedsPixels::Airlight)?;
        Ok(local::airlight_from_proxy((*proxy).clone(), info, s, frame))
    }
}
