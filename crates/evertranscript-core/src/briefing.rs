//! The Briefing: what this product does, said plainly, once, before it does
//! anything.
//!
//! ADR-0007 gives this product a **tool posture**. Consent for recording
//! Participants is the Operator's legal obligation, and the product does not
//! enforce, attest, or disclose on their behalf. In exchange the ADR
//! promises three legible things, and this is the first of them: a blunt
//! one-time briefing ending in an explicit acknowledgment, with nothing
//! captured before it.
//!
//! **The text below is the deliverable.** The modal that shows it is
//! packaging, and can be rewritten freely; the sentences cannot, because
//! each one exists to say a cost some ADR accepted eyes-open:
//!
//! - Recording law is the Operator's problem (ADR-0007).
//! - Voiceprints are stored for people who never agreed (ADR-0008).
//! - Auto-Record is on by default (ADR-0023).
//! - Copies of the History folder carry that biometric data (ADR-0035's own
//!   stated consequence).
//!
//! Softening any of them would improve acknowledgment rates and turn this
//! into a dark pattern with a consent button. If a future edit makes this
//! text more comfortable, that edit is the bug.
//!
//! **This is legal copy and it has not been reviewed by counsel.** The PRD
//! says review is mandatory before v1 and calls it per-jurisdiction work
//! rather than translation. [`AWAITING_COUNSEL`] says so in the product, and
//! removing that line is a decision for whoever commissions the review.

/// The English Briefing.
///
/// Written to be read once, by somebody deciding whether to trust a
/// recorder. Short sentences, no reassurance, no marketing.
pub const BRIEFING_EN: &str = "\
# Before you record anything

EverTranscript records meetings, transcribes them, and learns to recognise \
voices. All of that happens on this machine. None of it makes the following \
your problem any less.

## Recording other people is regulated, and that is on you

In many places it is a crime to record a conversation without the consent of \
everyone in it. In others one participant's consent is enough. Which rule \
applies depends on where you are, where the other people are, and sometimes \
on what is being discussed.

EverTranscript does not ask anyone for consent, does not announce itself in \
meetings, and does not keep evidence that you obtained permission. It is a \
recorder. Deciding whether you may lawfully use it, and telling the people in \
the room, is yours to do.

## It builds a voice profile for everyone it hears

To recognise the same person across meetings, EverTranscript stores a \
mathematical fingerprint of each voice — including people who never agreed to \
that and will never know it happened. In some jurisdictions a voiceprint is \
regulated biometric data and collecting one is the regulated act, whether or \
not it stays on your machine.

You can see every voice profile it holds, and delete any of them, in the Voice \
Registry. Deleting one stops recognition and changes nothing about what was \
said.

## Auto-Record is ON

Once set up, EverTranscript starts recording by itself when a meeting app is \
running and your microphone goes live. It does not ask first. That is the \
point of the product, and it means meetings will be recorded that you did not \
consciously decide to record.

There is one switch that turns this off, in Settings.

## Your recordings are files, and files travel

Everything lives in a folder you can open, copy, and back up. That is \
deliberate — the record is yours and nothing holds it hostage. It also means \
**a copy of that folder contains the voice profiles too**. Sending it to \
someone, syncing it, or backing it up takes the biometric data with it.

## What it does not do

It does not index your files, read your contacts, or look at your screen. It \
reads your calendar only if you grant that, and only the calendar already on \
this machine — never a cloud calendar account.

**Recording and transcribing on this machine is not the same as being \
silent.** Nothing said in a meeting leaves — not the audio, not the \
transcript, not the voice profiles — unless you explicitly choose a cloud \
service for summaries, and then it is the text of those meetings that goes. \
That is a promise about your meetings, not about the network. Separately, this \
app makes three kinds of call: an update check you can switch off, the model \
downloads it needs to work — the first ones start on their own, the rest when \
you ask — and that cloud summary service if you chose one. Nothing else.

The source is open. You can check all of this rather than believe it.";

/// The Simplified Chinese Briefing.
///
/// **A translation, not a jurisdiction-specific briefing**, and the product
/// says so where it is shown. Chinese-law recording and personal-information
/// rules (PIPL treats biometric data as sensitive personal information and
/// generally requires separate consent) are not what the English text
/// describes, and pretending a translation covers them would be worse than
/// showing English.
pub const BRIEFING_ZH: &str = "\
# 在开始录音之前

EverTranscript 会录制会议、转写内容，并学习识别说话人。这些都在本机完成。\
但这并不能减轻下面这些属于你的责任。

## 录制他人是受法律约束的，责任在你

在许多地方，未经在场所有人同意而录制对话属于犯罪；在另一些地方，一方同意即可。\
适用哪条规则，取决于你在哪里、其他人在哪里，有时还取决于谈话内容。

EverTranscript 不会替你征求任何人的同意，不会在会议中自报身份，也不会保存你已获得\
许可的证据。它只是一个录音工具。判断你是否可以合法使用它、并告知在场的人，是你的事。

## 它会为听到的每个人建立声音特征

为了在不同会议之间识别同一个人，EverTranscript 会保存每个声音的数学特征——包括那些\
从未同意、也永远不会知道此事的人。在部分法域中，声纹属于受监管的生物识别信息，\
采集本身即是受监管的行为，无论它是否只留在你的机器上。

你可以在「声音档案」中查看它保存的全部声音特征并逐条删除。删除只会停止识别，\
不会改变任何已记录的内容。

## 自动录制默认开启

设置完成后，只要检测到会议应用正在运行且麦克风被占用，EverTranscript 就会自行开始\
录制，不会事先询问。这正是本产品的设计意图，也意味着会有一些你并未有意决定录制的\
会议被录了下来。

设置中有一个开关可以关闭它。

## 你的录音是文件，而文件是会流动的

所有内容都存放在一个你可以打开、复制和备份的文件夹里。这是刻意的设计——记录属于你，\
不受任何人扣留。这同时意味着**该文件夹的副本也包含声音特征**。发送、同步或备份它，\
都会一并带走这些生物识别数据。

## 它不做的事

它不会索引你的文件、不会读取通讯录、不会查看屏幕内容。只有在你授权后它才会读取日历，\
且只读取本机上已有的日历，绝不访问云端日历账户。它唯一会通过网络发送的内容是：\
一个你可以关闭的更新检查、由你触发的模型下载，以及——当你明确选择云端摘要服务时——\
那些会议的文本。

源代码是公开的。以上每一条你都可以自行核实，而不必选择相信。";

/// Shown with any translated Briefing.
///
/// The PRD calls the Briefing's legal copy per-jurisdiction counsel work
/// rather than translation, and no counsel has read either version. Saying
/// so is cheaper than being wrong about someone's local law, and an Operator
/// reading a translated legal notice deserves to know it is a translation of
/// a notice written about somewhere else.
pub const AWAITING_COUNSEL: &str = "\
This notice has not been reviewed by a lawyer, and it describes obligations in \
general terms rather than under the law where you are. Translations of it are \
translations of the same general text, not a summary of local law.";

/// Which language a Client asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BriefingLanguage {
    English,
    SimplifiedChinese,
}

/// The Briefing, in one language, with the counsel disclaimer attached.
pub fn briefing(language: BriefingLanguage) -> String {
    let body = match language {
        BriefingLanguage::English => BRIEFING_EN,
        BriefingLanguage::SimplifiedChinese => BRIEFING_ZH,
    };
    format!("{body}\n\n---\n\n{AWAITING_COUNSEL}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four costs the Briefing exists to disclose, as the phrases a
    /// reader would actually look for.
    ///
    /// Not a style check. Each of these corresponds to an ADR that accepted
    /// a real cost eyes-open, and a Briefing that stopped mentioning one
    /// would be the product quietly withdrawing a disclosure it promised to
    /// make.
    #[test]
    fn the_english_briefing_says_all_four_things_it_must() {
        let text = BRIEFING_EN;
        assert!(
            text.contains("crime") && text.contains("consent"),
            "ADR-0007: recording law is the Operator's obligation"
        );
        assert!(
            text.contains("voice profile") && text.contains("never agreed"),
            "ADR-0008: voiceprints are stored for people who did not consent"
        );
        assert!(
            text.contains("Auto-Record is ON"),
            "ADR-0023: on by default, and it must be unmissable"
        );
        assert!(
            text.contains("contains the voice profiles too"),
            "ADR-0035: copies of the History folder carry biometric data"
        );
    }

    #[test]
    fn the_chinese_briefing_says_all_four_too() {
        let text = BRIEFING_ZH;
        assert!(
            text.contains("犯罪") && text.contains("同意"),
            "consent law"
        );
        assert!(
            text.contains("声音特征") && text.contains("从未同意"),
            "voiceprints of non-consenting people"
        );
        assert!(text.contains("自动录制默认开启"), "Auto-Record default");
        assert!(text.contains("生物识别数据"), "copies carry biometrics");
    }

    #[test]
    fn it_does_not_reassure() {
        // The failure mode for this text is not being wrong; it is being
        // comfortable. Marketing language here would raise acknowledgment
        // rates and make the acknowledgment worth less.
        for softener in [
            "we respect your privacy",
            "peace of mind",
            "rest assured",
            "don't worry",
            "completely safe",
            "100%",
        ] {
            assert!(
                !BRIEFING_EN.to_lowercase().contains(softener),
                "the Briefing must not reassure: found {softener:?}"
            );
        }
    }

    #[test]
    fn it_does_not_claim_the_product_handles_consent_for_you() {
        // ADR-0007's whole posture. A Briefing that implied the product
        // obtains, records, or attests consent would be describing a
        // different product and shifting a legal obligation the Operator
        // still has.
        assert!(BRIEFING_EN.contains("does not ask anyone for consent"));
        assert!(BRIEFING_EN.contains("yours to do"));
    }

    #[test]
    fn every_briefing_carries_the_counsel_disclaimer() {
        // No lawyer has read either version, and an Operator reading a legal
        // notice deserves to know that.
        for language in [
            BriefingLanguage::English,
            BriefingLanguage::SimplifiedChinese,
        ] {
            assert!(briefing(language).contains("has not been reviewed by a lawyer"));
        }
    }

    #[test]
    fn the_two_versions_cover_the_same_sections() {
        // A translation that quietly dropped a section would leave a
        // Chinese-reading Operator less informed than an English-reading
        // one about the same product.
        let count = |text: &str| text.lines().filter(|line| line.starts_with("## ")).count();
        assert_eq!(count(BRIEFING_EN), count(BRIEFING_ZH));
        assert_eq!(
            count(BRIEFING_EN),
            5,
            "four disclosures plus what it does not do"
        );
    }

    #[test]
    fn it_names_the_one_switch_that_turns_auto_record_off() {
        // Telling someone a product records by itself without telling them
        // how to stop it is a warning, not a disclosure.
        assert!(BRIEFING_EN.contains("one switch that turns this off"));
        assert!(BRIEFING_ZH.contains("开关可以关闭它"));
    }

    #[test]
    fn it_points_at_something_checkable() {
        // Story 46: the claims should be verifiable rather than trusted,
        // and the Briefing is where an evaluator is first told they can.
        assert!(BRIEFING_EN.contains("source is open"));
        assert!(BRIEFING_EN.contains("check all of this rather than believe it"));
    }
}
